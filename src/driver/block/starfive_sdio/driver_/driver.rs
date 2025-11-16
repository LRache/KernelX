use core::time::Duration;
use core::sync::atomic::{AtomicU16, Ordering};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::format;

use crate::driver::{BlockDriverOps, DeviceType, DriverOps};
use crate::kernel::event::timer::{wait_until, spin_delay};
use crate::{arch, kdebug, kinfo, kwarn};

use super::reg::*;
use super::cmd::*;
use super::err::*;
use super::sd_reg::*;

pub struct Driver {
    num: i32,
    base: usize,
    rca: AtomicU16,
}

impl Driver {
    pub fn new(num: i32, base: usize) -> Self {
        Driver { num, base, rca: AtomicU16::new(0) }
    }

    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }

    fn write_reg(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, val); }
    }

    fn wait_for_cmd_line(&self) -> Result<(), Timeout> {
        if wait_until(Duration::from_millis(0xFF), || {
            self.read_reg(REG_CMD) & CmdMask::start_cmd.bits() == 0
        }) {
            Ok(())
        } else {
            Err(Timeout::WaitCmdLine)
        }
    }

    fn wait_for_cmd_done(&self) -> Result<(), Timeout> {
        if wait_until(Duration::from_millis(0xFF), || {
            self.read_reg(REG_RINTSTS) & InterruptMask::cmd.bits() != 0
        }) {
            Ok(())
        } else {
            Err(Timeout::WaitCmdDone)
        }
    }

    fn wait_for_data_line(&self) -> Result<(), Timeout> {
        if wait_until(Duration::from_millis(DATA_TMOUT_DEFUALT as u64 * 1000), || {
            self.read_reg(REG_STATUS) & StatusMask::data_busy.bits() == 0
        }) {
            Ok(())
        } else {
            Err(Timeout::WaitDataLine)
        }
    }

    fn wait_reset(&self, mask: u32) -> Result<(), Timeout> {
        if wait_until(Duration::from_millis(10), || {
            self.read_reg(REG_CTRL) & mask == 0
        }) {
            Ok(())
        } else {
            Err(Timeout::WaitReset)
        }
    }

    fn reset_clock(&self, ena: u32, div: u32) -> Result<(), Timeout> {
        self.wait_for_cmd_line()?;
        self.write_reg(REG_CLKENA, 0);
        self.write_reg(REG_CLKDIV, div);
        
        let cmd = up_clk();
        self.write_reg(REG_CMDARG, cmd.arg());
        self.write_reg(REG_CMD, cmd.to_cmd());
        if ena == 0 {
            return Ok(());
        }
        
        self.wait_for_cmd_line()?;
        self.write_reg(REG_CMD, cmd.to_cmd());
        
        self.wait_for_cmd_line()?;
        self.write_reg(REG_CLKENA, ena);
        self.write_reg(REG_CMDARG, 0);
        self.write_reg(REG_CMD, cmd.to_cmd());
        kdebug!("reset clock");
        
        Ok(())
    }

    fn send_cmd(&self, cmd: Command) -> Result<Response, CardError> {
        loop {
            self.wait_for_data_line()?;
            self.wait_for_cmd_line()?;
            self.write_reg(REG_RINTSTS, InterruptMask::all().bits());
            self.write_reg(REG_CMDARG, cmd.arg());
            self.write_reg(REG_CMD, cmd.to_cmd());
            if self.read_reg(REG_RINTSTS) & InterruptMask::hle.bits() == 0 {
                kdebug!("Send CMD {:?}", CmdMask::from_bits(cmd.to_cmd()).unwrap());
                break;
            }
        }
        kdebug!(
            "{:?}",
            InterruptMask::from_bits(self.read_reg(REG_RINTSTS)).unwrap()
        );
        kdebug!("{:?}", StatusMask::from_bits(self.read_reg(REG_STATUS)).unwrap());
        self.wait_for_cmd_done()?;
        let resp = if cmd.resp_exp() {
            let mask: u32 = self.read_reg(REG_RINTSTS);
            if mask & InterruptMask::rto.bits() != 0 {
                self.write_reg(REG_RINTSTS, mask);
                kwarn!(
                    "Response Timeout, mask: {:?}",
                    InterruptMask::from_bits(mask).unwrap()
                );
                return Err(Interrupt::ResponseTimeout.into());
            } else if mask & InterruptMask::re.bits() != 0 {
                self.write_reg(REG_RINTSTS, mask);
                kwarn!(
                    "Response Error, mask : {:?}",
                    InterruptMask::from_bits(mask).unwrap()
                );
                return Err(Interrupt::ResponseErr.into());
            }
            if cmd.resp_lang() {
                let resp0 = self.read_reg(REG_RESP0);
                let resp1 = self.read_reg(REG_RESP1);
                let resp2 = self.read_reg(REG_RESP2);
                let resp3 = self.read_reg(REG_RESP3);
                Response::R136((resp0, resp1, resp2, resp3))
            } else {
                Response::R48(self.read_reg(REG_RESP0))
            }
        } else {
            Response::Rz
        };
        if cmd.data_exp() {
            self.wait_reset(ControlMask::fifo_reset.bits())?;
            self.write_reg(REG_BLKSIZ, BLKSIZ_DEFAULT);
            self.write_reg(REG_BYTCNT, BLKSIZ_DEFAULT);
        }

        Ok(resp)
    }

    fn check_version(&self) -> Result<(), CardError> {
        let cmd = send_if_cond(1, 0xAA);
        let cic = self.send_cmd(cmd)?.cic();
        if cic.voltage_accepted() == 1 && cic.pattern() == 0xAA {
            kdebug!("sd vision 2.0");
            spin_delay(Duration::from_millis(10));
            Ok(())
        } else {
            Err(CardError::VoltagePattern)
        }
    }

    fn check_v18_sdhc(&self) -> Result<(), CardError> {
        loop {
            let cmd = app_cmd(0);
            let status = self.send_cmd(cmd)?.card_status();
            kdebug!("{status:?}");
            let cmd = sd_send_op_cond(true, true);
            let ocr = self.send_cmd(cmd)?.ocr();
            if !ocr.is_busy() {
                if ocr.high_capacity() {
                    kdebug!("card is high capacity!");
                }
                if ocr.v18_allowed() {
                    kdebug!("card can switch to 1.8 voltage!");
                }
                break;
            }
            spin_delay(Duration::from_millis(10));
        }
        spin_delay(Duration::from_millis(10));
        Ok(())
    }

    fn check_rca(&self) -> Result<Rca, CardError> {
        let cmd = send_relative_address();
        let rca = self.send_cmd(cmd)?.rca();
        kdebug!("{:?}", rca);
        spin_delay(Duration::from_millis(10));
        Ok(rca)
    }

    fn check_csd(&self, rca: Rca) -> Result<(), CardError> {
        let cmd = send_csd(rca.address());
        let csd = self.send_cmd(cmd)?.csd();
        kdebug!("{:?}", csd);
        spin_delay(Duration::from_millis(10));
        Ok(())
    }

    fn check_cid(&self) -> Result<(), CardError> {
        let cmd = all_send_cid();
        let cid = self.send_cmd(cmd)?.cid();
        kdebug!("{:?}", cid);
        spin_delay(Duration::from_millis(10));
        Ok(())
    }

    fn function_switch(&self, arg: u32) -> Result<(), CardError> {
        let cmd = switch_function(arg);
        let status = self.send_cmd(cmd)?.card_status();
        kdebug!("{:?}", status);
        spin_delay(Duration::from_millis(10));
        Ok(())
    }

    fn set_bus(&self, rca: Rca) -> Result<(), CardError> {
        self.send_cmd(app_cmd(rca.address()))?;
        let status = self.send_cmd(set_bus_width(2))?.card_status();
        kdebug!("{:?}", status);
        spin_delay(Duration::from_millis(10));
        Ok(())
}

    fn sel_card(&self, rca: Rca) -> Result<(), CardError> {
        let cmd = select_card(rca.address());
        let status = self.send_cmd(cmd)?.card_status();
        kdebug!("{:?}", status);
        spin_delay(Duration::from_millis(10));
        Ok(())
    }

    pub fn init(&self) -> Result<(), CardError> {
        let hconf = HardConf::from(self.read_reg(REG_HCON));
        kdebug!("{:?}", hconf);
        
        // Reset Control Register
        let reset_mask = ControlMask::controller_reset.bits()
            | ControlMask::fifo_reset.bits()
            | ControlMask::dma_reset.bits();
        self.write_reg(REG_CTRL, reset_mask);
        self.wait_reset(reset_mask)?;

        // Enable power
        self.write_reg(REG_PWREN, 1);
        self.reset_clock(1, 62)?;
        self.write_reg(REG_TMOUT, 0xFFFFFFFF);
        
        // setup interrupt mask
        self.write_reg(REG_RINTSTS, InterruptMask::all().bits());
        self.write_reg(REG_INTMASK, 0);
        self.write_reg(REG_CTYPE, 1);
        self.write_reg(REG_BMOD, 1);
        
        // Enumerate card stack
        self.send_cmd(idle())?;
        spin_delay(Duration::from_millis(10));
        // delay(Duration::from_millis(10));
        
        self.check_version()?;
        self.check_v18_sdhc()?;
        self.check_cid()?;
        let rca = self.check_rca()?;
        self.rca.store(rca.address(), Ordering::Relaxed);
        self.check_csd(rca)?;
        self.sel_card(rca)?;
        self.function_switch(16777201)?;
        self.set_bus(rca)?;
        self.reset_clock(1, 1)?;
        
        kinfo!("sdio init success!");
        Ok(())
    }

    fn stop_transmission_ops(&self) -> Result<(), CardError> {
        let cmd = stop_transmission();
        loop {
            self.wait_for_cmd_line()?;
            self.write_reg(REG_RINTSTS, InterruptMask::all().bits());
            self.write_reg(REG_CMDARG, cmd.arg());
            self.write_reg(REG_CMD, cmd.to_cmd());
            if self.read_reg(REG_RINTSTS) & InterruptMask::hle.bits() == 0 {
                kdebug!("send {:?}", CmdMask::from_bits(cmd.to_cmd()).unwrap());
                break;
            }
        }
        let status = Response::R48(self.read_reg(REG_RESP0)).card_status();
        kdebug!("{status:?}");
        self.wait_for_cmd_done()?;
        Ok(())
    }

    fn fifo_cnt(&self) -> u32 {
        let status = self.read_reg(REG_STATUS);
        (status >> 17) & 0x1FFF
    }

    fn read_fifo(&self, offset: usize) -> u8 {
        let addr = (self.base + 0x200 + offset) as *mut u8;
        unsafe { addr.read_volatile() }
    }

    fn write_fifo(&self, offset: usize, val: u8) {
        let addr = (self.base + 0x200 + offset) as *mut u8;
        unsafe {
            addr.write_volatile(val);
        }
    }

    fn read_data(&self, buf: &mut [u8; BLKSIZ_DEFAULT as usize]) -> Result<(), CardError> {
        let mut offset = 0;
        // let timer = Timer::start(Duration::from_micros(DATA_TMOUT_DEFUALT as u64));
        let deadline = arch::get_time_us() + DATA_TMOUT_DEFUALT as u64;
        loop {
            let mask = self.read_reg(REG_RINTSTS);
            if offset == BLKSIZ_DEFAULT as usize && InterruptMask::dto.bits() & mask != 0 {
                break;
            }
            Interrupt::check(mask)?;
            spin_delay(Duration::from_micros(10));

            if arch::get_time_us() > deadline {
                return Err(CardError::DataTransferTimeout);
            }
            
            if mask & InterruptMask::rxdr.bits() != 0 || mask & InterruptMask::dto.bits() != 0 {
                while self.fifo_cnt() > 1 {
                    buf[offset] = self.read_fifo(offset);
                    offset += 1;
                }
                self.write_reg(REG_RINTSTS, InterruptMask::rxdr.bits());
            }
        }
        self.write_reg(REG_RINTSTS, self.read_reg(REG_RINTSTS));
        Ok(())
    }

    fn write_data(&self, buf: &[u8; BLKSIZ_DEFAULT as usize]) -> Result<(), CardError> {
        // let timer = Timer::start(Duration::from_micros(DATA_TMOUT_DEFUALT as u64));
        let deadline = arch::get_time_us() + DATA_TMOUT_DEFUALT as u64;
        loop {
            let mask = self.read_reg(REG_RINTSTS);
            if InterruptMask::dto.bits() & mask != 0 {
                break;
            }
            Interrupt::check(mask)?;
            spin_delay(Duration::from_micros(10));
            if arch::get_time_us() > deadline {
                return Err(CardError::DataTransferTimeout);
            }
            if mask & InterruptMask::txdr.bits() != 0 {
                for offset in 0..BLKSIZ_DEFAULT as usize {
                    self.write_fifo(offset, buf[offset])
                }
                self.write_reg(REG_RINTSTS, InterruptMask::txdr.bits());
            }
        }
        self.write_reg(REG_RINTSTS, self.read_reg(REG_RINTSTS));
        Ok(())
    }
}

impl BlockDriverOps for Driver {
    fn read_block(&self, block: usize, buf: &mut [u8]) -> Result<(), ()> {
        debug_assert!(buf.len() == BLKSIZ_DEFAULT as usize);

        kdebug!("read block {}", block);

        let cmd = read_single_block(block as u32);
        match self.send_cmd(cmd) {
            Ok(resp) => {
                let status = resp.card_status();
                kdebug!("{status:?}");
                if self.read_data(buf.try_into().unwrap()).is_err() {
                    self.stop_transmission_ops().map_err(|_| ())
                } else {
                    kdebug!("read block {} success, buf={:x?}", block, &buf[..32]);
                    Ok(())
                }
            }
            Err(err) => {
                kdebug!("{err:?}");
                self.stop_transmission_ops().map_err(|_| ())
            }
        }
    }

    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()> {
        kdebug!("write block {}", block);
        debug_assert!(buf.len() == BLKSIZ_DEFAULT as usize);
        
        let cmd = write_single_block(block as u32);
        match self.send_cmd(cmd) {
            Ok(resp) => {
                let status = resp.card_status();
                kdebug!("{status:?}");
                if self.write_data(buf.try_into().unwrap()).is_err() {
                    self.stop_transmission_ops().map_err(|_| ())
                } else {
                    kdebug!("write block {} success", block);
                    Ok(())
                }
            }
            Err(err) => {
                kdebug!("{err:?}");
                self.stop_transmission_ops().map_err(|_| ())
            }
        }
    }

    fn flush(&self) -> Result<(), ()> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), ()> {
        Ok(())
    }

    fn get_block_size(&self) -> u32 {
        BLKSIZ_DEFAULT
    }

    fn get_block_count(&self) -> u64 {
        0
    }
}

impl DriverOps for Driver {
    fn name(&self) -> &str {
        "starfive_sdio"
    }

    fn device_name(&self) -> String {
        format!("sdio{}", self.num)
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn as_block_driver(self: Arc<Self>) -> Arc<dyn BlockDriverOps> {
        self
    }
}
