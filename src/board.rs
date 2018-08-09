use adc_signal::{AdcChannel, AdcSampleTime, AdcSignal};
use core::fmt::Write;
use cortex_m;
use dac_mcp49xx::Mcp49xx;
use ms_timer::MsTimer;
use nucleo_f767zi::can::{Can1, Can2};
use nucleo_f767zi::debug_console::DebugConsole;
use nucleo_f767zi::hal::delay::Delay;
use nucleo_f767zi::hal::prelude::*;
use nucleo_f767zi::hal::serial::Serial;
use nucleo_f767zi::hal::stm32f7x7;
use nucleo_f767zi::hal::stm32f7x7::{ADC1, C_ADC, RCC};
use nucleo_f767zi::led::Leds;
use sh::hio;

// TODO - is this needed
pub use types::*;

// feature to pick how to route up debug_println/println?
// or
// - println! -> Serial3 console (currently debug_console)
// - debug_println! -> ITM/semihosting link

const CAN_PUBLISH_HZ: u32 = 50;

pub struct Board {
    pub semihost_console: hio::HStdout,
    pub debug_console: DebugConsole,
    pub leds: Leds,
    pub delay: Delay,
    pub timer_ms: MsTimer,
    pub can_publish_timer: CanPublishTimer,
    pub dac: Mcp49xx,
    pub control_can: ControlCan,
    pub obd_can: ObdCan,
    adc1: ADC1,
    brake_pins: BrakePins,
    throttle_pins: ThrottlePins,
    steering_pins: SteeringPins,
}

impl Board {
    pub fn new() -> Self {
        let mut semihost_console = hio::hstdout().unwrap();
        writeln!(semihost_console, "System starting");

        let mut core_peripherals = cortex_m::Peripherals::take().unwrap();
        let peripherals = stm32f7x7::Peripherals::take().unwrap();

        core_peripherals.SCB.enable_icache();
        core_peripherals
            .SCB
            .enable_dcache(&mut core_peripherals.CPUID);

        let mut flash = peripherals.FLASH.constrain();
        let mut rcc = peripherals.RCC.constrain();
        let mut adc1 = peripherals.ADC1;
        let mut c_adc = peripherals.C_ADC;

        let gpiob = peripherals.GPIOB.split(&mut rcc.ahb1);
        let mut gpioa = peripherals.GPIOA.split(&mut rcc.ahb1);
        let mut gpioc = peripherals.GPIOC.split(&mut rcc.ahb1);
        let mut gpiod = peripherals.GPIOD.split(&mut rcc.ahb1);
        let mut gpiof = peripherals.GPIOF.split(&mut rcc.ahb1);

        let brake_pins = BrakePins {
            spoof_enable: gpiod
                .pd12
                .into_push_pull_output(&mut gpiod.moder, &mut gpiod.otyper),
            brake_light_enable: gpiod
                .pd13
                .into_push_pull_output(&mut gpiod.moder, &mut gpiod.otyper),
            pedal_pos_sensor_high: gpiof
                .pf5
                .into_analog_input(&mut gpiof.moder, &mut gpiof.pupdr),
            pedal_pos_sensor_low: gpiof
                .pf10
                .into_analog_input(&mut gpiof.moder, &mut gpiof.pupdr),
        };

        let throttle_pins = ThrottlePins {
            spoof_enable: gpiod
                .pd10
                .into_push_pull_output(&mut gpiod.moder, &mut gpiod.otyper),
            accel_pos_sensor_high: gpioa
                .pa3
                .into_analog_input(&mut gpioa.moder, &mut gpioa.pupdr),
            accel_pos_sensor_low: gpioc
                .pc0
                .into_analog_input(&mut gpioc.moder, &mut gpioc.pupdr),
        };

        let steering_pins = SteeringPins {
            spoof_enable: gpiod
                .pd11
                .into_push_pull_output(&mut gpiod.moder, &mut gpiod.otyper),
            torque_sensor_high: gpioc
                .pc3
                .into_analog_input(&mut gpioc.moder, &mut gpioc.pupdr),
            torque_sensor_low: gpiof
                .pf3
                .into_analog_input(&mut gpiof.moder, &mut gpiof.pupdr),
        };

        let usart3_tx = gpiod.pd8.into_af7(&mut gpiod.moder, &mut gpiod.afrh);
        let usart3_rx = gpiod.pd9.into_af7(&mut gpiod.moder, &mut gpiod.afrh);

        // default clock configuration runs at 16 MHz
        let clocks = rcc.cfgr.freeze(&mut flash.acr);
        //
        // TODO - alternate clock configuration, breaks delay currently
        // need to check timer impl as well with this change
        //let clocks = rcc.cfgr.sysclk(64.mhz()).pclk1(32.mhz()).freeze(&mut
        // flash.acr);

        // TODO - need to push this down into the HAL in order to access
        // the constained RCC periphals
        // configure the ADCs
        init_adc(&mut adc1, &mut c_adc);

        // TODO - use the safe APIs once this block solidifies
        unsafe {
            // TODO - move this constant into BSP crate?
            // unlock registers to enable DWT cycle counter for MsTimer
            core_peripherals.DWT.lar.write(0xC5ACCE55);
        }

        writeln!(semihost_console, "sysclk = {} Hz", clocks.sysclk().0);
        writeln!(semihost_console, "pclk1 = {} Hz", clocks.pclk1().0);
        writeln!(semihost_console, "pclk2 = {} Hz", clocks.pclk2().0);

        let mut leds = Leds::new(gpiob);
        for led in leds.iter_mut() {
            led.off();
        }

        // USART3 is routed up to the same USB port as the stlink
        // shows up as /dev/ttyACM0 for me
        let serial = Serial::usart3(
            peripherals.USART3,
            (usart3_tx, usart3_rx),
            115_200.bps(),
            clocks,
            &mut rcc.apb1,
        );

        Board {
            semihost_console,
            debug_console: DebugConsole::new(serial),
            leds,
            delay: Delay::new(core_peripherals.SYST, clocks),
            timer_ms: MsTimer::new(core_peripherals.DWT, clocks),
            can_publish_timer: CanPublishTimer::tim2(
                peripherals.TIM2,
                CAN_PUBLISH_HZ.hz(),
                clocks,
                &mut rcc.apb1,
            ),
            dac: Mcp49xx::new(),
            control_can: Can1::new(),
            obd_can: Can2::new(),
            adc1,
            brake_pins,
            throttle_pins,
            steering_pins,
        }
    }

    pub fn brake_spoof_enable(&mut self) -> &mut BrakeSpoofEnablePin {
        &mut self.brake_pins.spoof_enable
    }

    pub fn brake_light_enable(&mut self) -> &mut BrakeLightEnablePin {
        &mut self.brake_pins.brake_light_enable
    }

    pub fn throttle_spoof_enable(&mut self) -> &mut ThrottleSpoofEnablePin {
        &mut self.throttle_pins.spoof_enable
    }

    pub fn steering_spoof_enable(&mut self) -> &mut SteeringSpoofEnablePin {
        &mut self.steering_pins.spoof_enable
    }

    pub fn anolog_read(&mut self, signal: AdcSignal, sample_time: AdcSampleTime) -> u16 {
        let channel = AdcChannel::from(signal);
        let smpt = u8::from(sample_time);

        // single conversion, uses the 1st conversion in the sequence
        self.adc1
            .sqr3
            .write(|w| unsafe { w.sq1().bits(u8::from(channel)) });

        // sample time in cycles
        // channel 10:18 uses SMPR1
        // channel 0:9 uses SMPR2
        match channel {
            AdcChannel::Adc123In3 => self.adc1.smpr2.write(|w| unsafe { w.smp3().bits(smpt) }),
            AdcChannel::Adc3In8 => self.adc1.smpr2.write(|w| unsafe { w.smp8().bits(smpt) }),
            AdcChannel::Adc3In9 => self.adc1.smpr2.write(|w| w.smp9().bits(smpt)),
            AdcChannel::Adc123In10 => self.adc1.smpr1.write(|w| unsafe { w.smp10().bits(smpt) }),
            AdcChannel::Adc123In13 => self.adc1.smpr1.write(|w| unsafe { w.smp13().bits(smpt) }),
            AdcChannel::Adc3In15 => self.adc1.smpr1.write(|w| unsafe { w.smp15().bits(smpt) }),
        };

        // start conversion
        self.adc1.cr2.modify(|_, w| w.swstart().set_bit());

        // wait for conversion to complete
        while !self.adc1.sr.read().eoc().bit() {}

        self.adc1.sr.modify(|_, w| {
            w
            // clear regular channel start flag
            .strt().clear_bit()
            // clear end of conversion flag
            .eoc().clear_bit()
        });

        self.adc1.dr.read().data().bits()
    }
}

// TODO - need to enable safe API bits in the HAL crate with config params
// DMA would be nice, to enable sequencing
fn init_adc(adc1: &mut ADC1, c_adc: &mut C_ADC) {
    let rcc = unsafe { &*RCC::ptr() };

    // ADC reset and release
    rcc.apb2rstr.modify(|_, w| w.adcrst().set_bit());
    rcc.apb2rstr.modify(|_, w| w.adcrst().clear_bit());

    // stop conversions while being configured
    adc1.cr2.modify(|_, w| w.swstart().clear_bit());

    // enable ADC1/2/3 peripheral clocks
    rcc.apb2enr
        .modify(|_, w| w.adc1en().set_bit().adc2en().set_bit().adc3en().set_bit());

    // TODO - need to update this once RCC is updated
    // set ADC prescaler, PCLK2 divided by 4
    c_adc.ccr.write(|w| unsafe { w.adcpre().bits(0b01) });

    adc1.cr1.write(|w| {
        w
            // disable overrun interrupt
            .ovrie().clear_bit()
            // 12-bit resolution
            .res().bits(0b00)
            // disable scan mode
            .scan().clear_bit()
            // disable analog watchdog
            .awden().clear_bit()
            .jawden().clear_bit()
            // disable end of conversion interrupt
            .eocie().clear_bit()
            // disable discontinuous mode
            .discen().clear_bit()
    });

    adc1.cr2.write(|w| {
        w
            // trigger detection disabled
            .exten().bits(0b00)
            // right alignment
            .align().clear_bit()
            // EOC set at the end of each regular conversion
            .eocs().set_bit()
            // disable continuous conversion mode
            .cont().clear_bit()
            // disable DMA
            .dds().clear_bit()
            .dma().clear_bit()
    });

    // single conversion
    adc1.sqr1.write(|w| w.l().bits(0b0000));

    // enable the ADC peripheral if needed, stabilizing if so
    if adc1.cr2.read().adon().bit() == false {
        adc1.cr2.modify(|_, w| w.adon().set_bit());

        // TODO - counter = (ADC_STAB_DELAY_US * (SystemCoreClock / 1000000));
        cortex_m::asm::delay(100);
    }

    // clear regular group conversion flag and overrun flag
    adc1.sr.modify(|_, w| w.ovr().clear_bit().eoc().clear_bit());
}
