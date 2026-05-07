use std::mem;
use std::time::{SystemTime, UNIX_EPOCH};

use num_enum::TryFromPrimitive;

use crate::device::bus::{Bus, MmioDevice};
use crate::dtb::{DtbBuilder, DtbConfig, dtb_node_name, dtb_reg_cells};

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum GoldfishRtcRegister {
    TimeLow = 0x00,
    TimeHigh = 0x04,
    AlarmLow = 0x08,
    AlarmHigh = 0x0c,
    IrqEnabled = 0x10,
    ClearAlarm = 0x14,
    AlarmStatus = 0x18,
    ClearInterrupt = 0x1c,
}

#[derive(Default)]
pub struct GoldfishRtcDevice {
    latched_time_high: Option<u32>,
    alarm: u64,
    irq_enabled: bool,
    alarm_status: bool,
    alarm_fired: bool,
}

impl GoldfishRtcDevice {
    pub const LENGTH: usize = 0x1000;

    fn now_nanos() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0)
    }

    fn is_u32_access(offset: usize, width: usize) -> bool {
        width == mem::size_of::<u32>() && (offset & (mem::size_of::<u32>() - 1)) == 0
    }

    fn set_alarm_low(&mut self, value: u32) {
        self.alarm = (self.alarm & !u64::from(u32::MAX)) | u64::from(value);
        self.alarm_status = false;
        self.alarm_fired = false;
    }

    fn set_alarm_high(&mut self, value: u32) {
        self.alarm = (self.alarm & u64::from(u32::MAX)) | (u64::from(value) << 32);
        self.alarm_status = false;
        self.alarm_fired = false;
    }
}

impl MmioDevice for GoldfishRtcDevice {
    fn read(&mut self, offset: usize, width: usize) -> Option<u64> {
        if !Self::is_u32_access(offset, width) {
            return None;
        }

        let value = match GoldfishRtcRegister::try_from(offset) {
            Ok(GoldfishRtcRegister::TimeLow) => {
                let now = Self::now_nanos();
                self.latched_time_high = Some((now >> 32) as u32);
                now as u32
            }
            Ok(GoldfishRtcRegister::TimeHigh) => self
                .latched_time_high
                .take()
                .unwrap_or_else(|| (Self::now_nanos() >> 32) as u32),
            Ok(GoldfishRtcRegister::AlarmLow) => self.alarm as u32,
            Ok(GoldfishRtcRegister::AlarmHigh) => (self.alarm >> 32) as u32,
            Ok(GoldfishRtcRegister::IrqEnabled) => u32::from(self.irq_enabled),
            Ok(GoldfishRtcRegister::ClearAlarm) => 0,
            Ok(GoldfishRtcRegister::AlarmStatus) => u32::from(self.alarm_status),
            Ok(GoldfishRtcRegister::ClearInterrupt) => 0,
            Err(_) => return None,
        };
        Some(u64::from(value))
    }

    fn write(&mut self, offset: usize, width: usize, value: u64) -> bool {
        if !Self::is_u32_access(offset, width) {
            return false;
        }

        let value = value as u32;
        match GoldfishRtcRegister::try_from(offset) {
            Ok(GoldfishRtcRegister::AlarmLow) => {
                self.set_alarm_low(value);
                true
            }
            Ok(GoldfishRtcRegister::AlarmHigh) => {
                self.set_alarm_high(value);
                true
            }
            Ok(GoldfishRtcRegister::IrqEnabled) => {
                self.irq_enabled = value != 0;
                true
            }
            Ok(GoldfishRtcRegister::ClearAlarm) => {
                if value != 0 {
                    self.alarm = 0;
                    self.alarm_status = false;
                    self.alarm_fired = false;
                }
                true
            }
            Ok(GoldfishRtcRegister::ClearInterrupt) => {
                if value != 0 {
                    self.alarm_status = false;
                }
                true
            }
            Ok(GoldfishRtcRegister::TimeLow)
            | Ok(GoldfishRtcRegister::TimeHigh)
            | Ok(GoldfishRtcRegister::AlarmStatus) => false,
            Err(_) => false,
        }
    }

    fn update(&mut self, _bus: &Bus) {
        if self.irq_enabled && self.alarm != 0 && !self.alarm_fired && Self::now_nanos() >= self.alarm {
            self.alarm_status = true;
            self.alarm_fired = true;
        }
    }

    fn interrupt_pending(&self) -> bool {
        self.irq_enabled && self.alarm_status
    }

    fn config_dtb(&self, builder: &mut DtbBuilder, config: &DtbConfig, addr: usize, len: usize, id: u32) {
        builder.begin_node(&dtb_node_name("rtc", addr));
        builder.prop_string("compatible", "google,goldfish-rtc");
        builder.prop_cells("reg", &dtb_reg_cells(addr, len));
        if id != 0 {
            builder.prop_u32("interrupt-parent", config.plic_phandle);
            builder.prop_u32("interrupts", id);
        }
        builder.end_node();
    }
}
