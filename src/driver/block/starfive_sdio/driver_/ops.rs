use core::sync::atomic::AtomicU16;
use core::sync::atomic::Ordering;
use core::time::Duration;

use crate::sd::cmd::*;
use crate::sd::reg::*;
use crate::sd::sd_reg::*;
use crate::sd::utils::*;
use crate::timer::delay;
use crate::timer::Timer;
use log::{debug, error, info};

use super::err::*;
static RCA: AtomicU16 = AtomicU16::new(0);

fn send_cmd(cmd: Command) -> Result<Response, CardError> {
    loop {
        wait_for_data_line()?;
        wait_for_cmd_line()?;
        write_reg(REG_RINTSTS, InterruptMask::all().bits());
        write_reg(REG_CMDARG, cmd.arg());
        write_reg(REG_CMD, cmd.to_cmd());
        if read_reg(REG_RINTSTS) & InterruptMask::hle.bits() == 0 {
            debug!("Send CMD {:?}", CmdMask::from_bits(cmd.to_cmd()).unwrap());
            break;
        }
    }
    debug!(
        "{:?}",
        InterruptMask::from_bits(read_reg(REG_RINTSTS)).unwrap()
    );
    debug!("{:?}", StatusMask::from_bits(read_reg(REG_STATUS)).unwrap());
    wait_for_cmd_done()?;
    let resp = if cmd.resp_exp() {
        let mask: u32 = read_reg(REG_RINTSTS);
        if mask & InterruptMask::rto.bits() != 0 {
            write_reg(REG_RINTSTS, mask);
            error!(
                "Response Timeout, mask: {:?}",
                InterruptMask::from_bits(mask).unwrap()
            );
            return Err(Interrupt::ResponseTimeout.into());
        } else if mask & InterruptMask::re.bits() != 0 {
            write_reg(REG_RINTSTS, mask);
            error!(
                "Response Error, mask : {:?}",
                InterruptMask::from_bits(mask).unwrap()
            );
            return Err(Interrupt::ResponseErr.into());
        }
        if cmd.resp_lang() {
            let resp0 = read_reg(REG_RESP0);
            let resp1 = read_reg(REG_RESP1);
            let resp2 = read_reg(REG_RESP2);
            let resp3 = read_reg(REG_RESP3);
            Response::R136((resp0, resp1, resp2, resp3))
        } else {
            Response::R48(read_reg(REG_RESP0))
        }
    } else {
        Response::Rz
    };
    if cmd.data_exp() {
        wait_reset(ControlMask::fifo_reset.bits())?;
        write_reg(REG_BLKSIZ, BLKSIZ_DEFAULT);
        write_reg(REG_BYTCNT, BLKSIZ_DEFAULT);
    }

    Ok(resp)
}

fn read_data(buf: &mut [u8; BLKSIZ_DEFAULT as usize]) -> Result<(), CardError> {
    let mut offset = 0;
    let timer = Timer::start(Duration::from_micros(DATA_TMOUT_DEFUALT as u64));
    loop {
        let mask = read_reg(REG_RINTSTS);
        if offset == BLKSIZ_DEFAULT as usize && InterruptMask::dto.bits() & mask != 0 {
            break;
        }
        Interrupt::check(mask)?;
        delay(Duration::from_micros(10));
        if timer.timeout() {
            return Err(CardError::DataTransferTimeout);
        }
        if mask & InterruptMask::rxdr.bits() != 0 || mask & InterruptMask::dto.bits() != 0 {
            while fifo_cnt() > 1 {
                buf[offset] = read_fifo(offset);
                offset += 1;
            }
            write_reg(REG_RINTSTS, InterruptMask::rxdr.bits());
        }
    }
    write_reg(REG_RINTSTS, read_reg(REG_RINTSTS));
    Ok(())
}

fn write_data(buf: &[u8; BLKSIZ_DEFAULT as usize]) -> Result<(), CardError> {
    let timer = Timer::start(Duration::from_micros(DATA_TMOUT_DEFUALT as u64));
    loop {
        let mask = read_reg(REG_RINTSTS);
        if InterruptMask::dto.bits() & mask != 0 {
            break;
        }
        Interrupt::check(mask)?;
        delay(Duration::from_micros(10));
        if timer.timeout() {
            return Err(CardError::DataTransferTimeout);
        }
        if mask & InterruptMask::txdr.bits() != 0 {
            for offset in 0..BLKSIZ_DEFAULT as usize {
                write_fifo(offset, buf[offset])
            }
            write_reg(REG_RINTSTS, InterruptMask::txdr.bits());
        }
    }
    write_reg(REG_RINTSTS, read_reg(REG_RINTSTS));
    Ok(())
}

fn reset_clock(ena: u32, div: u32) -> Result<(), Timeout> {
    wait_for_cmd_line()?;
    write_reg(REG_CLKENA, 0);
    write_reg(REG_CLKDIV, div);
    let cmd = up_clk();
    write_reg(REG_CMDARG, cmd.arg());
    write_reg(REG_CMD, cmd.to_cmd());
    if ena == 0 {
        return Ok(());
    }
    wait_for_cmd_line()?;
    write_reg(REG_CMD, cmd.to_cmd());
    wait_for_cmd_line()?;
    write_reg(REG_CLKENA, ena);
    write_reg(REG_CMDARG, 0);
    write_reg(REG_CMD, cmd.to_cmd());
    debug!("reset clock");
    Ok(())
}

pub(crate) fn init_card() -> Result<(), CardError> {
    info!("init sdio...");
    let hconf = HardConf::from(read_reg(REG_HCON));
    debug!("{hconf:?}");
    // Reset Control Register
    let reset_mask = ControlMask::controller_reset.bits()
        | ControlMask::fifo_reset.bits()
        | ControlMask::dma_reset.bits();
    write_reg(REG_CTRL, reset_mask);
    wait_reset(reset_mask)?;
    // enable power
    write_reg(REG_PWREN, 1);
    reset_clock(1, 62)?;
    write_reg(REG_TMOUT, 0xFFFFFFFF);
    // setup interrupt mask
    write_reg(REG_RINTSTS, InterruptMask::all().bits());
    write_reg(REG_INTMASK, 0);
    write_reg(REG_CTYPE, 1);
    write_reg(REG_BMOD, 1);
    // // enumerate card stack
    send_cmd(idle())?;
    delay(Duration::from_millis(10));
    check_version()?;
    check_v18_sdhc()?;
    check_cid()?;
    let rca = check_rca()?;
    RCA.store(rca.address(), Ordering::Relaxed);
    check_csd(rca)?;
    sel_card(rca)?;
    function_switch(16777201)?;
    set_bus(rca)?;
    reset_clock(1, 1)?;
    info!("sdio init success!");
    Ok(())
}

fn check_version() -> Result<(), CardError> {
    let cmd = send_if_cond(1, 0xAA);
    let cic = send_cmd(cmd)?.cic();
    if cic.voltage_accepted() == 1 && cic.pattern() == 0xAA {
        debug!("sd vision 2.0");
        delay(Duration::from_millis(10));
        Ok(())
    } else {
        Err(CardError::VoltagePattern)
    }
}

fn check_v18_sdhc() -> Result<(), CardError> {
    loop {
        let cmd = app_cmd(0);
        let status = send_cmd(cmd)?.card_status();
        debug!("{status:?}");
        let cmd = sd_send_op_cond(true, true);
        let ocr = send_cmd(cmd)?.ocr();
        if !ocr.is_busy() {
            if ocr.high_capacity() {
                debug!("card is high capacity!");
            }
            if ocr.v18_allowed() {
                debug!("card can switch to 1.8 voltage!");
            }
            break;
        }
        delay(Duration::from_millis(10));
    }
    delay(Duration::from_millis(10));
    Ok(())
}

fn check_rca() -> Result<Rca, CardError> {
    let cmd = send_relative_address();
    let rca = send_cmd(cmd)?.rca();
    debug!("{:?}", rca);
    delay(Duration::from_millis(10));
    Ok(rca)
}

fn check_cid() -> Result<(), CardError> {
    let cmd = all_send_cid();
    let cid = send_cmd(cmd)?.cid();
    debug!("{:?}", cid);
    delay(Duration::from_millis(10));
    Ok(())
}

fn check_csd(rca: Rca) -> Result<(), CardError> {
    let cmd = send_csd(rca.address());
    let csd = send_cmd(cmd)?.csd();
    debug!("{:?}", csd);
    delay(Duration::from_millis(10));
    Ok(())
}

fn sel_card(rca: Rca) -> Result<(), CardError> {
    let cmd = select_card(rca.address());
    let status = send_cmd(cmd)?.card_status();
    debug!("{:?}", status);
    delay(Duration::from_millis(10));
    Ok(())
}

fn function_switch(arg: u32) -> Result<(), CardError> {
    let cmd = switch_function(arg);
    let status = send_cmd(cmd)?.card_status();
    debug!("{:?}", status);
    delay(Duration::from_millis(10));
    Ok(())
}

fn set_bus(rca: Rca) -> Result<(), CardError> {
    send_cmd(app_cmd(rca.address()))?;
    let status = send_cmd(set_bus_width(2))?.card_status();
    debug!("{:?}", status);
    delay(Duration::from_millis(10));
    Ok(())
}

fn stop_transmission_ops() -> Result<(), CardError> {
    let cmd = stop_transmission();
    loop {
        wait_for_cmd_line()?;
        write_reg(REG_RINTSTS, InterruptMask::all().bits());
        write_reg(REG_CMDARG, cmd.arg());
        write_reg(REG_CMD, cmd.to_cmd());
        if read_reg(REG_RINTSTS) & InterruptMask::hle.bits() == 0 {
            debug!("send {:?}", CmdMask::from_bits(cmd.to_cmd()).unwrap());
            break;
        }
    }
    let status = Response::R48(read_reg(REG_RESP0)).card_status();
    debug!("{status:?}");
    wait_for_cmd_done()?;
    Ok(())
}

pub(crate) fn read_block(buf: &mut [u8; 512], addr: u32) -> Result<(), CardError> {
    let cmd = read_single_block(addr);
    match send_cmd(cmd) {
        Ok(resp) => {
            let status = resp.card_status();
            debug!("{status:?}");
            if read_data(buf).is_err() {
                stop_transmission_ops()
            } else {
                Ok(())
            }
        }
        Err(err) => {
            debug!("{err:?}");
            stop_transmission_ops()
        }
    }
}

pub(crate) fn write_block(buf: &[u8; BLKSIZ_DEFAULT as usize], addr: u32) -> Result<(), CardError> {
    let cmd = write_single_block(addr);
    match send_cmd(cmd) {
        Ok(resp) => {
            let status = resp.card_status();
            debug!("{status:?}");
            if write_data(buf).is_err() {
                stop_transmission_ops()
            } else {
                Ok(())
            }
        }
        Err(err) => {
            debug!("{err:?}");
            stop_transmission_ops()
        }
    }
}
