//! A driver to interface with the MCP4922 (12-bit, dual channel DAC)
//!
//! This driver was built using [`embedded-hal`] traits.
//!
//! This is a minimal port of the Arduino implementation
//! used in OSCC. You can find the original source here:
//! https://github.com/jonlamb-gh/oscc/blob/master/firmware/common/libs/DAC_MCP49xx/README.txt
//!
//! Features that don't exist:
//! - latching
//! - gain configuration
//! - reference voltage configuration

use embedded_hal::blocking::spi::Write;
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::{Mode, Phase, Polarity};

use ranges::Bounded;
use typenum::{U0, U1, U4096};

type U4095 = op! { U4096 - U1 };

/// It's a 12 bit dac, so the upper bound is 4095 (2^12 - 1)
pub type DacOutput = Bounded<u16, U0, U4095>;

/// SPI mode
pub const MODE: Mode = Mode {
    phase: Phase::CaptureOnFirstTransition,
    polarity: Polarity::IdleLow,
};

/// DAC Channel
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Channel {
    ChannelA,
    ChannelB,
}

/// DAC Errors
#[derive(Debug)]
pub enum Error<E> {
    /// SPI error
    Spi(E),
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::Spi(e)
    }
}

/// MCP4922 driver
pub struct Mcp4922<SPI, CS> {
    spi: SPI,
    cs: CS,
}

impl<SPI, CS, E> Mcp4922<SPI, CS>
where
    SPI: Write<u8, Error = E>,
    CS: OutputPin,
{
    /// Creates a new driver from a SPI peripheral and a CS pin
    pub fn new(spi: SPI, mut cs: CS) -> Self {
        // unselect the device
        cs.set_high();

        Mcp4922 { spi, cs }
    }

    /// Writes the two output values to the two output channels of the DAC
    pub fn output_ab(&mut self, output_a: DacOutput, output_b: DacOutput) -> Result<(), E> {
        self.output(output_a, Channel::ChannelA)?;
        self.output(output_b, Channel::ChannelB)
    }

    /// Writes a bounded 16-bit value `data` to the output `channel` of the DAC
    pub fn output(&mut self, data: DacOutput, channel: Channel) -> Result<(), E> {
        self.cs.set_low();

        // NOTE: swapping the bytes here, the HAL should be able to handle such a thing
        let mut buffer = [0u8; 2];
        // bits 11 through 0: data
        buffer[1] = (data.val() & 0x00FF) as _;
        buffer[0] = ((data.val() >> 8) & (0x000F as u16)) as u8
            // bit 12: shutdown bit. 1 for active operation
            | (1 << 4)
            // bit 13: gain bit; 0 for 1x gain, 1 for 2x
            // bit 14: buffer VREF?
            // bit 15: 0 for DAC A, 1 for DAC B
            | u8::from(channel) << 7;

        if let Err(e) = self.spi.write(&buffer) {
            self.cs.set_high();
            return Err(e);
        }

        self.cs.set_high();

        Ok(())
    }
}

impl From<Channel> for u8 {
    fn from(c: Channel) -> u8 {
        match c {
            Channel::ChannelA => 0b0,
            Channel::ChannelB => 0b1,
        }
    }
}
