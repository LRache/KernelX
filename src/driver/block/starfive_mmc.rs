use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

use bitflags::bitflags;

use crate::arch::map_kernel_addr;
use crate::driver::{BlockDriverOps, Device, DeviceType, DriverOps, MMIOMatcher as MMIOMatcherTrait};
use crate::kernel::event::timer;
use crate::kernel::mm::{MapPerm, page};
use crate::klib::SpinLock;
use crate::{arch, kinfo, kwarn};

const SDIO1_BASE: usize = 0x1602_0000;
const BLOCK_SIZE: usize = 512;
const DEFAULT_CIU_HZ: u32 = 50_000_000;
const IDENTIFICATION_HZ: u32 = 400_000;
// TODO: Raise this after implementing JH7110 sampling-phase configuration and tuning.
const DEFAULT_TRANSFER_HZ: u32 = 6_250_000;

#[derive(Clone, Copy, Debug)]
enum Error {
    InvalidBuffer,
    OutOfRange,
    UnsupportedCard,
    InvalidCapacity,
    Timeout(&'static str),
    Command { index: u8, status: u32 },
    Data { index: u8, status: u32 },
}

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidBuffer => formatter.write_str("invalid block buffer"),
            Self::OutOfRange => formatter.write_str("block address is out of range"),
            Self::UnsupportedCard => formatter.write_str("unsupported SD card"),
            Self::InvalidCapacity => formatter.write_str("invalid SD card capacity"),
            Self::Timeout(operation) => write!(formatter, "timeout during {operation}"),
            Self::Command { index, status } => {
                write!(formatter, "CMD{index} failed with RINTSTS={status:#x}")
            }
            Self::Data { index, status } => {
                write!(formatter, "CMD{index} data failed with RINTSTS={status:#x}")
            }
        }
    }
}

type Result<T> = core::result::Result<T, Error>;

#[repr(usize)]
#[derive(Clone, Copy)]
enum Register {
    Control = 0x000,
    PowerEnable = 0x004,
    ClockDivider = 0x008,
    ClockSource = 0x00c,
    ClockEnable = 0x010,
    Timeout = 0x014,
    CardType = 0x018,
    BlockSize = 0x01c,
    ByteCount = 0x020,
    InterruptMask = 0x024,
    CommandArgument = 0x028,
    Command = 0x02c,
    Response0 = 0x030,
    Response1 = 0x034,
    Response2 = 0x038,
    Response3 = 0x03c,
    RawInterruptStatus = 0x044,
    Status = 0x048,
    FifoThreshold = 0x04c,
    VersionId = 0x06c,
    HardwareConfig = 0x070,
    BusMode = 0x080,
}

bitflags! {
    #[derive(Clone, Copy)]
    struct Control: u32 {
        const CONTROLLER_RESET = 1 << 0;
        const FIFO_RESET = 1 << 1;
        const DMA_RESET = 1 << 2;
        const INTERRUPT_ENABLE = 1 << 4;
        const DMA_ENABLE = 1 << 5;
        const USE_IDMAC = 1 << 25;
    }

    #[derive(Clone, Copy)]
    struct CommandFlags: u32 {
        const RESPONSE_EXPECTED = 1 << 6;
        const RESPONSE_LONG = 1 << 7;
        const RESPONSE_CRC = 1 << 8;
        const DATA_EXPECTED = 1 << 9;
        const DATA_WRITE = 1 << 10;
        const WAIT_PREVIOUS_DATA = 1 << 13;
        const STOP_ABORT = 1 << 14;
        const SEND_INITIALIZATION = 1 << 15;
        const UPDATE_CLOCK = 1 << 21;
        const USE_HOLD_REGISTER = 1 << 29;
        const START = 1 << 31;
    }

    #[derive(Clone, Copy)]
    struct Interrupt: u32 {
        const RESPONSE_ERROR = 1 << 1;
        const COMMAND_DONE = 1 << 2;
        const DATA_OVER = 1 << 3;
        const TX_FIFO_REQUEST = 1 << 4;
        const RX_FIFO_REQUEST = 1 << 5;
        const RESPONSE_CRC_ERROR = 1 << 6;
        const DATA_CRC_ERROR = 1 << 7;
        const RESPONSE_TIMEOUT = 1 << 8;
        const DATA_TIMEOUT = 1 << 9;
        const HOST_TIMEOUT = 1 << 10;
        const FIFO_ERROR = 1 << 11;
        const HARDWARE_LOCKED = 1 << 12;
        const START_BIT_ERROR = 1 << 13;
        const END_BIT_ERROR = 1 << 15;

        const COMMAND_ERRORS = Self::RESPONSE_ERROR.bits()
            | Self::RESPONSE_CRC_ERROR.bits()
            | Self::RESPONSE_TIMEOUT.bits()
            | Self::HARDWARE_LOCKED.bits();
        const DATA_ERRORS = Self::DATA_CRC_ERROR.bits()
            | Self::DATA_TIMEOUT.bits()
            | Self::HOST_TIMEOUT.bits()
            | Self::FIFO_ERROR.bits()
            | Self::HARDWARE_LOCKED.bits()
            | Self::START_BIT_ERROR.bits()
            | Self::END_BIT_ERROR.bits();
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum Command {
    GoIdle = 0,
    AllSendCid = 2,
    SendRelativeAddress = 3,
    SelectCard = 7,
    SendInterfaceCondition = 8,
    SendCsd = 9,
    SetBlockLength = 16,
    ReadSingleBlock = 17,
    WriteSingleBlock = 24,
    AppCommand = 55,
    SetBusWidth = 6,
    SdSendOperatingCondition = 41,
}

#[derive(Clone, Copy)]
enum ResponseType {
    None,
    Short,
    ShortBusy,
    Long,
    OperatingCondition,
}

impl ResponseType {
    fn flags(self) -> CommandFlags {
        match self {
            Self::None => CommandFlags::empty(),
            Self::Short | Self::ShortBusy => CommandFlags::RESPONSE_EXPECTED | CommandFlags::RESPONSE_CRC,
            Self::Long => CommandFlags::RESPONSE_EXPECTED | CommandFlags::RESPONSE_LONG | CommandFlags::RESPONSE_CRC,
            Self::OperatingCondition => CommandFlags::RESPONSE_EXPECTED,
        }
    }
}

#[derive(Clone, Copy)]
enum DataDirection {
    Read,
    Write,
}

#[derive(Clone, Copy)]
enum FifoWidth {
    Bits16,
    Bits32,
    Bits64,
}

impl FifoWidth {
    fn bytes(self) -> usize {
        match self {
            Self::Bits16 => 2,
            Self::Bits32 => 4,
            Self::Bits64 => 8,
        }
    }
}

#[derive(Clone, Copy)]
struct Card {
    relative_address: u32,
    high_capacity: bool,
    block_count: u64,
}

struct Host {
    base: usize,
    ciu_hz: u32,
    transfer_hz: u32,
    bus_width: u32,
    fifo_depth: usize,
    fifo_offset: usize,
    fifo_width: FifoWidth,
    card: Option<Card>,
}

impl Host {
    fn new(base: usize, ciu_hz: u32, transfer_hz: u32, bus_width: u32, fifo_depth: usize) -> Self {
        Self {
            base,
            ciu_hz,
            transfer_hz,
            bus_width,
            fifo_depth,
            fifo_offset: 0,
            fifo_width: FifoWidth::Bits32,
            card: None,
        }
    }

    fn read_reg(&self, register: Register) -> u32 {
        let address = (self.base + register as usize) as *const u32;
        // SAFETY: `base` is a mapped JH7110 MMC MMIO range and every register
        // passed here is a naturally aligned 32-bit register in that range.
        unsafe { arch::read_volatile(address) }
    }

    fn write_reg(&mut self, register: Register, value: u32) {
        let address = (self.base + register as usize) as *mut u32;
        // SAFETY: `base` is a mapped JH7110 MMC MMIO range and every register
        // passed here is a naturally aligned 32-bit register in that range.
        unsafe { arch::write_volatile(address, value) }
    }

    fn read_fifo(&self) -> u64 {
        let address = self.base + self.fifo_offset;
        match self.fifo_width {
            FifoWidth::Bits16 => {
                // SAFETY: `fifo_offset` is selected from VERID and HCON reports
                // that the FIFO requires naturally aligned 16-bit accesses.
                unsafe { arch::read_volatile(address as *const u16) as u64 }
            }
            FifoWidth::Bits32 => {
                // SAFETY: `fifo_offset` is selected from VERID and HCON reports
                // that the FIFO requires naturally aligned 32-bit accesses.
                unsafe { arch::read_volatile(address as *const u32) as u64 }
            }
            FifoWidth::Bits64 => {
                // SAFETY: `fifo_offset` is selected from VERID and HCON reports
                // that the FIFO requires naturally aligned 64-bit accesses.
                unsafe { arch::read_volatile(address as *const u64) }
            }
        }
    }

    fn write_fifo(&mut self, value: u64) {
        let address = self.base + self.fifo_offset;
        match self.fifo_width {
            FifoWidth::Bits16 => {
                // SAFETY: `fifo_offset` is selected from VERID and HCON reports
                // that the FIFO requires naturally aligned 16-bit accesses.
                unsafe { arch::write_volatile(address as *mut u16, value as u16) }
            }
            FifoWidth::Bits32 => {
                // SAFETY: `fifo_offset` is selected from VERID and HCON reports
                // that the FIFO requires naturally aligned 32-bit accesses.
                unsafe { arch::write_volatile(address as *mut u32, value as u32) }
            }
            FifoWidth::Bits64 => {
                // SAFETY: `fifo_offset` is selected from VERID and HCON reports
                // that the FIFO requires naturally aligned 64-bit accesses.
                unsafe { arch::write_volatile(address as *mut u64, value) }
            }
        }
    }

    fn wait_for(duration: Duration, condition: impl FnMut() -> bool, what: &'static str) -> Result<()> {
        if timer::wait_until(duration, condition) {
            Ok(())
        } else {
            Err(Error::Timeout(what))
        }
    }

    fn initialize_controller(&mut self) -> Result<()> {
        let version = self.read_reg(Register::VersionId) & 0xffff;
        self.fifo_offset = if version < 0x240a { 0x100 } else { 0x200 };
        self.fifo_width = match (self.read_reg(Register::HardwareConfig) >> 7) & 0x7 {
            0 => FifoWidth::Bits16,
            2 => FifoWidth::Bits64,
            _ => FifoWidth::Bits32,
        };

        self.write_reg(Register::PowerEnable, 1);
        self.write_reg(Register::InterruptMask, 0);
        self.write_reg(Register::RawInterruptStatus, u32::MAX);
        self.write_reg(Register::Timeout, u32::MAX);

        let mut control = Control::from_bits_retain(self.read_reg(Register::Control));
        control.remove(Control::INTERRUPT_ENABLE | Control::DMA_ENABLE | Control::USE_IDMAC);
        control.insert(Control::CONTROLLER_RESET | Control::FIFO_RESET | Control::DMA_RESET);
        self.write_reg(Register::Control, control.bits());
        Self::wait_for(
            Duration::from_millis(100),
            || {
                self.read_reg(Register::Control)
                    & (Control::CONTROLLER_RESET | Control::FIFO_RESET | Control::DMA_RESET).bits()
                    == 0
            },
            "controller reset",
        )?;

        self.write_reg(Register::BusMode, 1);
        Self::wait_for(
            Duration::from_millis(100),
            || self.read_reg(Register::BusMode) & 1 == 0,
            "IDMAC reset",
        )?;

        let half_depth = self.fifo_depth / 2;
        let fifo_threshold = (2 << 28) | ((half_depth.saturating_sub(1) as u32) << 16) | half_depth as u32;
        self.write_reg(Register::FifoThreshold, fifo_threshold);
        self.write_reg(Register::CardType, 0);
        self.set_clock(IDENTIFICATION_HZ)
    }

    fn send_clock_update(&mut self) -> Result<()> {
        Self::wait_for(
            Duration::from_millis(500),
            || self.read_reg(Register::Command) & CommandFlags::START.bits() == 0,
            "previous command",
        )?;
        self.write_reg(Register::CommandArgument, 0);
        self.write_reg(
            Register::Command,
            (CommandFlags::START | CommandFlags::UPDATE_CLOCK | CommandFlags::WAIT_PREVIOUS_DATA).bits(),
        );
        Self::wait_for(
            Duration::from_millis(500),
            || self.read_reg(Register::Command) & CommandFlags::START.bits() == 0,
            "clock update",
        )?;
        let status = Interrupt::from_bits_retain(self.read_reg(Register::RawInterruptStatus));
        self.write_reg(Register::RawInterruptStatus, status.bits());
        if status.contains(Interrupt::HARDWARE_LOCKED) {
            Err(Error::Command {
                index: 0,
                status: status.bits(),
            })
        } else {
            Ok(())
        }
    }

    fn set_clock(&mut self, requested_hz: u32) -> Result<()> {
        let requested_hz = requested_hz.min(self.ciu_hz).max(1);
        let divider = if requested_hz == self.ciu_hz {
            0
        } else {
            self.ciu_hz.div_ceil(requested_hz).div_ceil(2)
        };
        if divider > u8::MAX as u32 {
            return Err(Error::UnsupportedCard);
        }

        self.write_reg(Register::ClockEnable, 0);
        self.write_reg(Register::ClockSource, 0);
        self.send_clock_update()?;
        self.write_reg(Register::ClockDivider, divider);
        self.send_clock_update()?;
        self.write_reg(Register::ClockEnable, 1);
        self.send_clock_update()
    }

    fn reset_fifo(&mut self) -> Result<()> {
        let control = Control::from_bits_retain(self.read_reg(Register::Control)) | Control::FIFO_RESET;
        self.write_reg(Register::Control, control.bits());
        Self::wait_for(
            Duration::from_millis(100),
            || self.read_reg(Register::Control) & Control::FIFO_RESET.bits() == 0,
            "FIFO reset",
        )
    }

    fn command(
        &mut self,
        command: Command,
        argument: u32,
        response_type: ResponseType,
        data: Option<DataDirection>,
    ) -> Result<[u32; 4]> {
        let index = command as u8;
        Self::wait_for(
            Duration::from_millis(500),
            || self.read_reg(Register::Command) & CommandFlags::START.bits() == 0,
            "command engine",
        )?;
        if data.is_some() {
            Self::wait_for(
                Duration::from_millis(500),
                || self.read_reg(Register::Status) & (1 << 9) == 0,
                "data busy",
            )?;
        }

        self.write_reg(Register::RawInterruptStatus, u32::MAX);
        self.write_reg(Register::CommandArgument, argument);
        let mut flags = CommandFlags::START | CommandFlags::USE_HOLD_REGISTER | response_type.flags();
        if matches!(command, Command::GoIdle) {
            flags |= CommandFlags::STOP_ABORT | CommandFlags::SEND_INITIALIZATION;
        }
        if let Some(direction) = data {
            flags |= CommandFlags::DATA_EXPECTED | CommandFlags::WAIT_PREVIOUS_DATA;
            if matches!(direction, DataDirection::Write) {
                flags |= CommandFlags::DATA_WRITE;
            }
        }
        self.write_reg(Register::Command, flags.bits() | index as u32);

        Self::wait_for(
            Duration::from_millis(500),
            || {
                let status = Interrupt::from_bits_retain(self.read_reg(Register::RawInterruptStatus));
                status.intersects(Interrupt::COMMAND_DONE | Interrupt::COMMAND_ERRORS)
            },
            "command response",
        )?;
        let status = Interrupt::from_bits_retain(self.read_reg(Register::RawInterruptStatus));
        self.write_reg(
            Register::RawInterruptStatus,
            (status & (Interrupt::COMMAND_DONE | Interrupt::COMMAND_ERRORS)).bits(),
        );
        if status.intersects(Interrupt::COMMAND_ERRORS) {
            return Err(Error::Command {
                index,
                status: status.bits(),
            });
        }

        let response = [
            self.read_reg(Register::Response3),
            self.read_reg(Register::Response2),
            self.read_reg(Register::Response1),
            self.read_reg(Register::Response0),
        ];
        if matches!(response_type, ResponseType::ShortBusy) {
            Self::wait_for(
                Duration::from_millis(500),
                || self.read_reg(Register::Status) & (1 << 9) == 0,
                "card busy",
            )?;
        }
        Ok(response)
    }

    fn fifo_count(&self) -> usize {
        ((self.read_reg(Register::Status) >> 17) & 0x1fff) as usize
    }

    fn pull_fifo(&self, buffer: &mut [u8], offset: &mut usize) {
        while self.fifo_count() != 0 && *offset < buffer.len() {
            let value = self.read_fifo();
            let count = self.fifo_width.bytes().min(buffer.len() - *offset);
            for byte in 0..count {
                buffer[*offset + byte] = (value >> (byte * 8)) as u8;
            }
            *offset += count;
        }
    }

    fn push_fifo(&mut self, buffer: &[u8], offset: &mut usize) {
        while self.fifo_count() < self.fifo_depth && *offset < buffer.len() {
            let count = self.fifo_width.bytes().min(buffer.len() - *offset);
            let mut value = 0;
            for byte in 0..count {
                value |= (buffer[*offset + byte] as u64) << (byte * 8);
            }
            self.write_fifo(value);
            *offset += count;
        }
    }

    fn finish_read(&mut self, command: Command, buffer: &mut [u8]) -> Result<()> {
        let mut offset = 0;
        if let Err(error) = Self::wait_for(
            Duration::from_millis(500),
            || {
                let status = Interrupt::from_bits_retain(self.read_reg(Register::RawInterruptStatus));
                if status.intersects(Interrupt::DATA_OVER | Interrupt::DATA_ERRORS) {
                    true
                } else if status.contains(Interrupt::RX_FIFO_REQUEST) {
                    self.pull_fifo(buffer, &mut offset);
                    self.write_reg(Register::RawInterruptStatus, Interrupt::RX_FIFO_REQUEST.bits());
                    false
                } else {
                    false
                }
            },
            "read data",
        ) {
            self.reset_fifo()?;
            return Err(error);
        }
        let status = Interrupt::from_bits_retain(self.read_reg(Register::RawInterruptStatus));
        if status.contains(Interrupt::DATA_OVER) {
            self.pull_fifo(buffer, &mut offset);
        }
        self.write_reg(Register::RawInterruptStatus, status.bits());
        if status.intersects(Interrupt::DATA_ERRORS) || offset != buffer.len() {
            self.reset_fifo()?;
            return Err(Error::Data {
                index: command as u8,
                status: status.bits(),
            });
        }
        Ok(())
    }

    fn finish_write(&mut self, command: Command, buffer: &[u8]) -> Result<()> {
        let mut offset = 0;
        if let Err(error) = Self::wait_for(
            Duration::from_millis(500),
            || {
                let status = Interrupt::from_bits_retain(self.read_reg(Register::RawInterruptStatus));
                if status.intersects(Interrupt::DATA_OVER | Interrupt::DATA_ERRORS) {
                    true
                } else if status.contains(Interrupt::TX_FIFO_REQUEST) {
                    self.push_fifo(buffer, &mut offset);
                    self.write_reg(Register::RawInterruptStatus, Interrupt::TX_FIFO_REQUEST.bits());
                    false
                } else {
                    false
                }
            },
            "write data",
        ) {
            self.reset_fifo()?;
            return Err(error);
        }
        let status = Interrupt::from_bits_retain(self.read_reg(Register::RawInterruptStatus));
        self.write_reg(Register::RawInterruptStatus, status.bits());
        if status.intersects(Interrupt::DATA_ERRORS) || offset != buffer.len() {
            self.reset_fifo()?;
            return Err(Error::Data {
                index: command as u8,
                status: status.bits(),
            });
        }
        Self::wait_for(
            Duration::from_millis(500),
            || self.read_reg(Register::Status) & (1 << 9) == 0,
            "write completion",
        )
    }

    fn initialize_card(&mut self) -> Result<Card> {
        self.initialize_controller()?;
        self.command(Command::GoIdle, 0, ResponseType::None, None)?;
        timer::spin_delay(Duration::from_millis(2));

        let version_two = match self.command(Command::SendInterfaceCondition, 0x1aa, ResponseType::Short, None) {
            Ok(response) if response[3] & 0xfff == 0x1aa => true,
            Ok(_) => return Err(Error::UnsupportedCard),
            Err(Error::Command { status, .. })
                if Interrupt::from_bits_retain(status).contains(Interrupt::RESPONSE_TIMEOUT) =>
            {
                false
            }
            Err(error) => return Err(error),
        };

        let operating_condition = self.wait_for_operating_condition(version_two)?;
        let high_capacity = operating_condition & (1 << 30) != 0;
        self.command(Command::AllSendCid, 0, ResponseType::Long, None)?;
        let relative_address = self.command(Command::SendRelativeAddress, 0, ResponseType::Short, None)?[3] >> 16;
        let csd = self.command(Command::SendCsd, relative_address << 16, ResponseType::Long, None)?;
        let block_count = Self::parse_block_count(csd)?;
        self.command(
            Command::SelectCard,
            relative_address << 16,
            ResponseType::ShortBusy,
            None,
        )?;

        if !high_capacity {
            self.command(Command::SetBlockLength, BLOCK_SIZE as u32, ResponseType::Short, None)?;
        }
        if self.bus_width >= 4 {
            self.command(Command::AppCommand, relative_address << 16, ResponseType::Short, None)?;
            self.command(Command::SetBusWidth, 2, ResponseType::Short, None)?;
            self.write_reg(Register::CardType, self.read_reg(Register::CardType) | 1);
        }
        self.set_clock(self.transfer_hz)?;

        let card = Card {
            relative_address,
            high_capacity,
            block_count,
        };
        self.card = Some(card);
        Ok(card)
    }

    fn wait_for_operating_condition(&mut self, version_two: bool) -> Result<u32> {
        let start = arch::get_time_us();
        loop {
            self.command(Command::AppCommand, 0, ResponseType::Short, None)?;
            let mut argument = 0x00ff_8000;
            if version_two {
                argument |= 1 << 30;
            }
            let response = self.command(
                Command::SdSendOperatingCondition,
                argument,
                ResponseType::OperatingCondition,
                None,
            )?[3];
            if response & (1 << 31) != 0 {
                return Ok(response);
            }
            if arch::get_time_us().saturating_sub(start) >= 1_000_000 {
                return Err(Error::Timeout("card initialization"));
            }
            timer::spin_delay(Duration::from_millis(10));
        }
    }

    fn parse_block_count(csd: [u32; 4]) -> Result<u64> {
        match csd[0] >> 30 {
            0 => {
                let read_block_length = (csd[1] >> 16) & 0xf;
                let device_size = ((csd[1] & 0x3ff) << 2) | (csd[2] >> 30);
                let size_multiplier = (csd[2] >> 15) & 0x7;
                let block_length = 1u64.checked_shl(read_block_length).ok_or(Error::InvalidCapacity)?;
                let multiplier = 1u64.checked_shl(size_multiplier + 2).ok_or(Error::InvalidCapacity)?;
                (device_size as u64 + 1)
                    .checked_mul(multiplier)
                    .and_then(|blocks| blocks.checked_mul(block_length))
                    .map(|bytes| bytes / BLOCK_SIZE as u64)
                    .filter(|blocks| *blocks != 0)
                    .ok_or(Error::InvalidCapacity)
            }
            1 => {
                let device_size = ((csd[1] & 0x3f) << 16) | (csd[2] >> 16);
                (device_size as u64 + 1).checked_mul(1024).ok_or(Error::InvalidCapacity)
            }
            _ => Err(Error::UnsupportedCard),
        }
    }

    fn card_argument(&self, block: usize) -> Result<u32> {
        let card = self.card.ok_or(Error::UnsupportedCard)?;
        if block as u64 >= card.block_count {
            return Err(Error::OutOfRange);
        }
        if card.high_capacity {
            u32::try_from(block).map_err(|_| Error::OutOfRange)
        } else {
            block
                .checked_mul(BLOCK_SIZE)
                .and_then(|address| u32::try_from(address).ok())
                .ok_or(Error::OutOfRange)
        }
    }

    fn read_block(&mut self, block: usize, buffer: &mut [u8]) -> Result<()> {
        if buffer.len() != BLOCK_SIZE {
            return Err(Error::InvalidBuffer);
        }
        let argument = self.card_argument(block)?;
        self.write_reg(Register::BlockSize, BLOCK_SIZE as u32);
        self.write_reg(Register::ByteCount, BLOCK_SIZE as u32);
        self.command(
            Command::ReadSingleBlock,
            argument,
            ResponseType::Short,
            Some(DataDirection::Read),
        )?;
        self.finish_read(Command::ReadSingleBlock, buffer)
    }

    fn write_block(&mut self, block: usize, buffer: &[u8]) -> Result<()> {
        if buffer.len() != BLOCK_SIZE {
            return Err(Error::InvalidBuffer);
        }
        let argument = self.card_argument(block)?;
        self.write_reg(Register::BlockSize, BLOCK_SIZE as u32);
        self.write_reg(Register::ByteCount, BLOCK_SIZE as u32);
        self.command(
            Command::WriteSingleBlock,
            argument,
            ResponseType::Short,
            Some(DataDirection::Write),
        )?;
        self.finish_write(Command::WriteSingleBlock, buffer)
    }
}

pub struct Driver {
    name: String,
    host: SpinLock<Host>,
    block_count: u64,
    readahead: AtomicUsize,
}

impl Driver {
    fn new(name: String, host: Host, block_count: u64) -> Self {
        Self {
            name,
            host: SpinLock::new(host, "starfive_mmc::host"),
            block_count,
            readahead: AtomicUsize::new(0),
        }
    }
}

impl DriverOps for Driver {
    fn name(&self) -> &str {
        "starfive_mmc"
    }

    fn device_name(&self) -> String {
        self.name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn as_block_driver(self: Arc<Self>) -> Option<Arc<dyn BlockDriverOps>> {
        Some(self)
    }
}

impl BlockDriverOps for Driver {
    fn read_block(&self, block: usize, buffer: &mut [u8]) -> core::result::Result<(), ()> {
        self.host.lock().read_block(block, buffer).map_err(|error| {
            kwarn!("starfive_mmc: block {} read failed: {}", block, error);
        })
    }

    fn write_block(&self, block: usize, buffer: &[u8]) -> core::result::Result<(), ()> {
        self.host.lock().write_block(block, buffer).map_err(|error| {
            kwarn!("starfive_mmc: block {} write failed: {}", block, error);
        })
    }

    fn get_block_size(&self) -> u32 {
        BLOCK_SIZE as u32
    }

    fn get_block_count(&self) -> u64 {
        self.block_count
    }

    fn get_readahead(&self) -> usize {
        self.readahead.load(Ordering::Relaxed)
    }

    fn set_readahead(&self, readahead: usize) {
        self.readahead.store(readahead, Ordering::Relaxed);
    }
}

fn read_u32_property(device: &Device, name: &str) -> Option<u32> {
    let value = device.fdt_node().property(name)?.value;
    let bytes: [u8; 4] = value.get(..4)?.try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}

fn is_enabled(device: &Device) -> bool {
    device
        .fdt_node()
        .property("status")
        .and_then(|property| property.as_str())
        .is_none_or(|status| matches!(status, "ok" | "okay"))
}

pub struct MMIOMatcher;

impl MMIOMatcherTrait for MMIOMatcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        device.match_compatible(&["starfive,jh7110-mmc", "snps,dw-mshc"])?;
        if !is_enabled(device) {
            return None;
        }
        let (mmio_base, mmio_size) = device.mmio()?;
        if mmio_base != SDIO1_BASE {
            return None;
        }

        let pages = arch::page_count(mmio_size);
        let kernel_base = page::alloc_contiguous(pages);
        map_kernel_addr(kernel_base, mmio_base, mmio_size, MapPerm::RW);

        let ciu_hz = read_u32_property(device, "assigned-clock-rates")
            .or_else(|| read_u32_property(device, "clock-frequency"))
            .unwrap_or(DEFAULT_CIU_HZ);
        let transfer_hz = read_u32_property(device, "max-frequency")
            .unwrap_or(DEFAULT_TRANSFER_HZ)
            .min(DEFAULT_TRANSFER_HZ);
        let bus_width = read_u32_property(device, "bus-width").unwrap_or(1);
        let fifo_depth = read_u32_property(device, "fifo-depth").unwrap_or(32) as usize;
        if fifo_depth < 2 {
            kwarn!("starfive_mmc: invalid FIFO depth {}", fifo_depth);
            return None;
        }

        let mut host = Host::new(kernel_base, ciu_hz, transfer_hz, bus_width, fifo_depth);
        match host.initialize_card() {
            Ok(card) => {
                kinfo!(
                    "starfive_mmc: initialized {} blocks, RCA={:#x}, high_capacity={}",
                    card.block_count,
                    card.relative_address,
                    card.high_capacity
                );
                Some(Arc::new(Driver::new(device.name().into(), host, card.block_count)))
            }
            Err(error) => {
                kwarn!("starfive_mmc: initialization failed: {}", error);
                None
            }
        }
    }
}
