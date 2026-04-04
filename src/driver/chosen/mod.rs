use alloc::collections::btree_map::BTreeMap;

use crate::driver::manager::get_rtc_driver;
use crate::{kinfo, kwarn};

pub mod kclock;
pub mod kconsole;
pub mod kpmu;

pub fn init(bootargs: &BTreeMap<&'static str, &'static str>) {
    if let Some(name) = bootargs.get("rtc") {
        if let Some(driver) = get_rtc_driver(&name) {
            kclock::register(driver);
            kinfo!("Chosen RTC driver '{}' registered", name);
        } else {
            kwarn!("Chosen RTC driver '{}' not found", name);
        }
    }
}
