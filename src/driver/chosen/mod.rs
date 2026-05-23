use alloc::collections::btree_map::BTreeMap;

use crate::driver::manager::get_rtc_driver;
use crate::fs::devfs::devnode::CharDevInode;
use crate::fs::vfs;
use crate::{kinfo, kwarn};

pub mod kclock;
pub mod kconsole;
pub mod kpmu;

pub fn init(bootargs: &BTreeMap<&'static str, &'static str>) {
    init_kconsole(bootargs);

    if let Some(name) = bootargs.get("rtc") {
        if let Some(driver) = get_rtc_driver(&name) {
            kclock::register(driver);
            kinfo!("Chosen RTC driver '{}' registered", name);
        } else {
            kwarn!("Chosen RTC driver '{}' not found", name);
        }
    }
}

fn init_kconsole(bootargs: &BTreeMap<&'static str, &'static str>) {
    let Some(name) = bootargs.get("kconsole") else {
        return;
    };

    if !name.starts_with("/dev/") {
        kwarn!("Chosen kernel console '{}' is not a /dev device", name);
        return;
    }

    match vfs::load_dentry(name) {
        Ok(dentry) => {
            let inode = dentry.get_inode();
            match inode.downcast_arc::<CharDevInode>() {
                Ok(inode) => {
                    kconsole::register_driver(inode.driver().clone());
                    kinfo!("Chosen kernel console '{}' registered", name);
                }
                Err(_) => {
                    kwarn!("Chosen kernel console '{}' is not a character device", name);
                }
            }
        }
        Err(err) => {
            kwarn!("Chosen kernel console '{}' not found: {:?}", name, err);
        }
    }
}
