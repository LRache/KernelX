use alloc::string::String;
use alloc::sync::Arc;
use core::convert::TryFrom;
use core::time::Duration;

use bitflags::bitflags;

use crate::arch;
use crate::driver::{Device, DeviceType, DriverOps, MMIOMatcher as MMIOMatcherTrait, RTCDriverOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::klib::SpinLock;

#[derive(Clone, Copy)]
#[repr(usize)]
enum Register {
    ToyWrite0 = 0x24,
    ToyWrite1 = 0x28,
    ToyRead0 = 0x2c,
    ToyRead1 = 0x30,
    RtcCtrl = 0x40,
    RtcRead0 = 0x68,
}

#[derive(Clone, Copy)]
enum ToyField {
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Subsecond,
}

impl ToyField {
    fn shift(self) -> u32 {
        match self {
            Self::Month => 26,
            Self::Day => 21,
            Self::Hour => 16,
            Self::Minute => 10,
            Self::Second => 4,
            Self::Subsecond => 0,
        }
    }

    fn mask(self) -> u32 {
        match self {
            Self::Month => 0x3f,
            Self::Day | Self::Hour => 0x1f,
            Self::Minute | Self::Second => 0x3f,
            Self::Subsecond => 0xf,
        }
    }

    fn get(self, value: u32) -> u32 {
        (value >> self.shift()) & self.mask()
    }

    fn set(self, value: u32) -> u32 {
        (value & self.mask()) << self.shift()
    }
}

bitflags! {
    struct CtrlFlags: u32 {
        const OSC_ENABLE = 1 << 8;
        const TOY_ENABLE = 1 << 11;
        const RTC_ENABLE = 1 << 13;
    }
}

const SECS_PER_DAY: u64 = 86400;
const NANOS_PER_SEC: u64 = 1_000_000_000;
const NANOS_PER_TOY_SUBSECOND: u32 = 100_000_000;
const RTC_COUNTER_FREQ: u64 = 32_768;
const RTC_COUNTER_WRAP: u64 = 1_u64 << 32;
const UNIX_EPOCH_YEAR: i64 = 1970;
const FIRST_YEAR: i64 = 2000;
const LAST_YEAR: i64 = 2099;

fn month_days(year: i64) -> [u32; 12] {
    [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ]
}

fn days_before_year(year: i64) -> i64 {
    let elapsed_years = year - UNIX_EPOCH_YEAR;
    365 * elapsed_years + leap_days_before(year) - leap_days_before(UNIX_EPOCH_YEAR)
}

fn leap_days_before(year: i64) -> i64 {
    let prev = year - 1;
    prev / 4 - prev / 100 + prev / 400
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[derive(Clone, Copy)]
struct RtcTime {
    subsecond: u32,
    second: u32,
    minute: u32,
    hour: u32,
    day: u32,
    month: u32,
    year: i64,
}

impl RtcTime {
    fn from_duration(time: Duration) -> SysResult<Self> {
        let secs = time.as_secs();
        let min_secs = days_before_year(FIRST_YEAR) as u64 * SECS_PER_DAY;
        let max_secs = days_before_year(LAST_YEAR + 1) as u64 * SECS_PER_DAY;
        if secs < min_secs || secs >= max_secs {
            return Err(Errno::EINVAL);
        }

        let mut days = i64::try_from(secs / SECS_PER_DAY).map_err(|_| Errno::EINVAL)?;
        let day_secs = secs % SECS_PER_DAY;

        let mut year = UNIX_EPOCH_YEAR;
        loop {
            let days_in_year = if is_leap(year) { 366 } else { 365 };
            if days < days_in_year {
                break;
            }
            days -= days_in_year;
            year += 1;
        }

        let month_days = month_days(year);
        let mut month = 1;
        for days_in_month in month_days {
            if days < i64::from(days_in_month) {
                break;
            }
            days -= i64::from(days_in_month);
            month += 1;
        }

        let time = Self {
            subsecond: time.subsec_nanos() / NANOS_PER_TOY_SUBSECOND,
            second: (day_secs % 60) as u32,
            minute: ((day_secs / 60) % 60) as u32,
            hour: (day_secs / 3600) as u32,
            day: days as u32 + 1,
            month,
            year,
        };
        time.check_range()?;
        Ok(time)
    }

    fn to_duration(self) -> SysResult<Duration> {
        self.check_range()?;

        let mut days = days_before_year(self.year);
        for days_in_month in month_days(self.year).iter().take(self.month as usize - 1) {
            days += i64::from(*days_in_month);
        }
        days += i64::from(self.day - 1);

        let seconds = days
            .checked_mul(SECS_PER_DAY as i64)
            .and_then(|seconds| seconds.checked_add(i64::from(self.hour) * 3600))
            .and_then(|seconds| seconds.checked_add(i64::from(self.minute) * 60))
            .and_then(|seconds| seconds.checked_add(i64::from(self.second)))
            .ok_or(Errno::EINVAL)?;

        Ok(Duration::new(
            u64::try_from(seconds).map_err(|_| Errno::EINVAL)?,
            self.subsecond * NANOS_PER_TOY_SUBSECOND,
        ))
    }

    fn check_range(self) -> SysResult<()> {
        if !(FIRST_YEAR..=LAST_YEAR).contains(&self.year)
            || self.month == 0
            || self.month > 12
            || self.hour >= 24
            || self.minute >= 60
            || self.second >= 60
            || self.subsecond >= 10
        {
            return Err(Errno::EINVAL);
        }

        let max_day = month_days(self.year)[self.month as usize - 1];
        if self.day == 0 || self.day > max_day {
            return Err(Errno::EINVAL);
        }

        Ok(())
    }
}

#[derive(Clone, Copy)]
struct ClockState {
    base_time: Duration,
    base_counter: u64,
    high_counter: u64,
    last_counter: u32,
}

pub struct Driver {
    base: usize,
    name: String,
    lock: SpinLock<ClockState>,
}

impl Driver {
    fn new(base: usize, name: String) -> Self {
        Self {
            base,
            name,
            lock: SpinLock::new(
                ClockState {
                    base_time: Duration::ZERO,
                    base_counter: 0,
                    high_counter: 0,
                    last_counter: 0,
                },
                "Ls7aRtc::lock",
            ),
        }
    }

    fn read(&self, register: Register) -> u32 {
        unsafe { arch::read_volatile((self.base + register as usize) as *const u32) }
    }

    fn write(&self, register: Register, value: u32) {
        unsafe { arch::write_volatile((self.base + register as usize) as *mut u32, value) };
    }

    fn set_enabled(&self) {
        let ctrl = CtrlFlags::from_bits_retain(self.read(Register::RtcCtrl));
        self.write(
            Register::RtcCtrl,
            (ctrl | CtrlFlags::OSC_ENABLE | CtrlFlags::TOY_ENABLE | CtrlFlags::RTC_ENABLE).bits(),
        );
    }

    fn check_enabled(&self, flags: CtrlFlags) -> SysResult<()> {
        let ctrl = CtrlFlags::from_bits_retain(self.read(Register::RtcCtrl));
        if ctrl.contains(CtrlFlags::OSC_ENABLE | flags) {
            Ok(())
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn read_time(&self) -> SysResult<RtcTime> {
        self.check_enabled(CtrlFlags::TOY_ENABLE)?;
        let low = self.read(Register::ToyRead0);
        let year = i64::from(self.read(Register::ToyRead1)) + 1900;

        Ok(RtcTime {
            subsecond: ToyField::Subsecond.get(low),
            second: ToyField::Second.get(low),
            minute: ToyField::Minute.get(low),
            hour: ToyField::Hour.get(low),
            day: ToyField::Day.get(low),
            month: ToyField::Month.get(low),
            year,
        })
    }

    fn write_time(&self, time: RtcTime) {
        let low = ToyField::Second.set(time.second)
            | ToyField::Subsecond.set(time.subsecond)
            | ToyField::Minute.set(time.minute)
            | ToyField::Hour.set(time.hour)
            | ToyField::Day.set(time.day)
            | ToyField::Month.set(time.month);

        self.set_enabled();
        self.write(Register::ToyWrite0, low);
        self.write(Register::ToyWrite1, (time.year - 1900) as u32);
    }

    fn read_counter(&self, state: &mut ClockState) -> u64 {
        let counter = self.read(Register::RtcRead0);
        if counter < state.last_counter {
            state.high_counter += RTC_COUNTER_WRAP;
        }
        state.last_counter = counter;
        state.high_counter + u64::from(counter)
    }

    fn init_time_base(&self) -> SysResult<()> {
        let base_time = self.read_time()?.to_duration()?;
        let mut state = self.lock.lock();
        let counter = self.read(Register::RtcRead0);
        state.high_counter = 0;
        state.last_counter = counter;
        state.base_counter = u64::from(counter);
        state.base_time = base_time;
        Ok(())
    }
}

impl DriverOps for Driver {
    fn name(&self) -> &str {
        "ls7a-rtc"
    }

    fn device_name(&self) -> String {
        self.name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Rtc
    }

    fn as_rtc_driver(self: Arc<Self>) -> Option<Arc<dyn RTCDriverOps>> {
        Some(self)
    }
}

impl RTCDriverOps for Driver {
    fn now(&self) -> SysResult<Duration> {
        self.check_enabled(CtrlFlags::RTC_ENABLE)?;
        let mut state = self.lock.lock();
        let counter = self.read_counter(&mut state);
        let elapsed_counter = counter.saturating_sub(state.base_counter);
        let elapsed_time = Duration::new(
            elapsed_counter / RTC_COUNTER_FREQ,
            ((elapsed_counter % RTC_COUNTER_FREQ) * NANOS_PER_SEC / RTC_COUNTER_FREQ) as u32,
        );
        state.base_time.checked_add(elapsed_time).ok_or(Errno::EINVAL)
    }

    fn set_time(&self, time: Duration) -> SysResult<()> {
        let rtc_time = RtcTime::from_duration(time)?;
        let mut state = self.lock.lock();
        self.write_time(rtc_time);
        let counter = self.read_counter(&mut state);
        state.base_counter = counter;
        state.base_time = time;
        Ok(())
    }
}

pub struct MMIOMatcher;

impl MMIOMatcherTrait for MMIOMatcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        device.match_compatible(&["loongson,ls7a-rtc"])?;

        let (mmio_base, mmio_size) = device.mmio()?;
        if mmio_size < Register::RtcRead0 as usize + core::mem::size_of::<u32>() {
            return None;
        }

        let driver = Arc::new(Driver::new(
            arch::mmio_phys_to_kaddr(mmio_base, mmio_size),
            device.name().into(),
        ));
        driver.set_enabled();
        driver.init_time_base().ok()?;
        Some(driver)
    }
}
