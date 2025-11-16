use crate::timer::Timer;
use core::time::Duration;

use super::{
    err::Timeout,
    reg::{
        CmdMask, InterruptMask, StatusMask, DATA_TMOUT_DEFUALT, REG_CMD, REG_CTRL, REG_RINTSTS,
        REG_STATUS,
    },
};

const SDIO_BASE: usize = 0x16020000;
#[inline]
pub(crate) fn write_reg(reg: u32, val: u32) {
    let addr = (SDIO_BASE + reg as usize) as *mut u32;
    unsafe {
        addr.write_volatile(val);
    }
}
#[inline]
pub(crate) fn read_reg(reg: u32) -> u32 {
    let addr = (SDIO_BASE + reg as usize) as *mut u32;
    unsafe { addr.read_volatile() }
}

pub(crate) fn wait_for<F: FnMut() -> bool>(dur: Duration, mut f: F) -> bool {
    let timer = Timer::start(dur);
    loop {
        if timer.timeout() {
            return false;
        }
        if f() {
            break;
        }
    }
    true
}

pub(crate) fn wait_for_cmd_line() -> Result<(), Timeout> {
    if !wait_for(Duration::from_millis(0xFF), || {
        read_reg(REG_CMD) & CmdMask::start_cmd.bits() == 0
    }) {
        Err(Timeout::WaitCmdLine)
    } else {
        Ok(())
    }
}

pub(crate) fn wait_for_data_line() -> Result<(), Timeout> {
    if wait_for(Duration::from_millis(DATA_TMOUT_DEFUALT as u64), || {
        read_reg(REG_STATUS) & StatusMask::data_busy.bits() == 0
    }) {
        Ok(())
    } else {
        Err(Timeout::WaitDataLine)
    }
}

pub(crate) fn wait_for_cmd_done() -> Result<(), Timeout> {
    if wait_for(Duration::from_millis(0xFF), || {
        read_reg(REG_RINTSTS) & InterruptMask::cmd.bits() != 0
    }) {
        Ok(())
    } else {
        Err(Timeout::WaitCmdDone)
    }
}

pub(crate) fn wait_reset(mask: u32) -> Result<(), Timeout> {
    if wait_for(Duration::from_millis(10), || read_reg(REG_CTRL) & mask == 0) {
        Ok(())
    } else {
        Err(Timeout::WaitReset)
    }
}

pub(crate) fn fifo_cnt() -> u32 {
    let status = read_reg(REG_STATUS);
    (status >> 17) & 0x1FFF
}

pub(crate) fn read_fifo(offset: usize) -> u8 {
    let addr = (SDIO_BASE + 0x200 + offset) as *mut u8;
    unsafe { addr.read_volatile() }
}
pub(crate) fn write_fifo(offset: usize, val: u8) {
    let addr = (SDIO_BASE + 0x200 + offset) as *mut u8;
    unsafe {
        addr.write_volatile(val);
    }
}
