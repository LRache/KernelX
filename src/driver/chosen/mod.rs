use alloc::collections::btree_map::BTreeMap;
use alloc::sync::Arc;

use crate::driver::CharDriverOps;
use crate::driver::manager::get_rtc_driver;
use crate::fs::devfs::devnode::CharDevInode;
use crate::fs::vfs;
use crate::{kinfo, kwarn};

pub mod kclock;
pub mod kconsole;
pub mod kdebug_console;
pub mod kpmu;

pub fn init(bootargs: &BTreeMap<&'static str, &'static str>) {
    init_kconsole(bootargs);
    init_kdebug_console(bootargs);

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
    init_chosen_char_device(bootargs, "kconsole", "kernel console", kconsole::register_driver);
}

fn init_kdebug_console(bootargs: &BTreeMap<&'static str, &'static str>) {
    init_chosen_char_device(
        bootargs,
        "kdebug_console",
        "kernel debug console",
        kdebug_console::register_driver,
    );
}

fn init_chosen_char_device(
    bootargs: &BTreeMap<&'static str, &'static str>,
    key: &'static str,
    description: &str,
    register: fn(Arc<dyn CharDriverOps>),
) {
    let Some(name) = bootargs.get(key) else {
        return;
    };
    if !name.starts_with("/dev/") {
        kwarn!("Chosen {} '{}' is not a /dev device", description, name);
        return;
    }

    match vfs::load_dentry(name) {
        Ok(dentry) => {
            let inode = dentry.get_inode();
            match inode.downcast_arc::<CharDevInode>() {
                Ok(inode) => {
                    register(inode.driver().clone());
                    kinfo!("Chosen {} '{}' registered", description, name);
                }
                Err(_) => {
                    kwarn!("Chosen {} '{}' is not a character device", description, name);
                }
            }
        }
        Err(err) => {
            kwarn!("Chosen {} '{}' not found: {:?}", description, name, err);
        }
    }
}
