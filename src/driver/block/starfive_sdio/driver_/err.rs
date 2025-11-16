use super::reg::InterruptMask;
use core::fmt::Debug;

#[derive(Debug, Clone, Copy)]
pub enum CardError {
    CardInitErr,
    InterruptErr(Interrupt),
    TimeoutErr(Timeout),
    VoltagePattern,
    DataTransferTimeout,
}

impl From<Timeout> for CardError {
    fn from(value: Timeout) -> Self {
        Self::TimeoutErr(value)
    }
}

impl From<Interrupt> for CardError {
    fn from(value: Interrupt) -> Self {
        Self::InterruptErr(value)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Timeout {
    WaitReset,
    WaitCmdLine,
    WaitCmdDone,
    WaitDataLine,
    FifoStatus,
}

#[derive(Debug, Clone, Copy)]
pub enum Interrupt {
    ResponseTimeout,
    ResponseErr,
    EndBitErr,
    StartBitErr,
    HardwareLock,
    Fifo,
    DataReadTimeout,
    DataCrc,
}

impl Interrupt {
    pub fn check(mask: u32) -> Result<(), Interrupt> {
        let mut ret = Ok(());

        if mask & InterruptMask::dcrc.bits() != 0 {
            ret = Err(Interrupt::DataCrc);
        }
        if mask & InterruptMask::drto.bits() != 0 {
            ret = Err(Interrupt::DataReadTimeout);
        }
        if mask & InterruptMask::frun.bits() != 0 {
            ret = Err(Interrupt::Fifo);
        }
        if mask & InterruptMask::hle.bits() != 0 {
            ret = Err(Interrupt::HardwareLock);
        }
        if mask & InterruptMask::sbe.bits() != 0 {
            ret = Err(Interrupt::StartBitErr);
        }
        if mask & InterruptMask::ebe.bits() != 0 {
            ret = Err(Interrupt::EndBitErr);
        }
        ret
    }
}
