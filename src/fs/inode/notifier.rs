use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::klib::SpinLock;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct InotifyEvent: u32 {
        const READ = 0x0000_0001;
        const MODIFY = 0x0000_0002;
        const ATTRIB = 0x0000_0004;
        const CLOSE_WRITE = 0x0000_0008;
        const CLOSE_NOWRITE = 0x0000_0010;
        const OPEN = 0x0000_0020;
        const MOVED_FROM = 0x0000_0040;
        const MOVED_TO = 0x0000_0080;
        const CREATE = 0x0000_0100;
        const DELETE = 0x0000_0200;
        const DELETE_SELF = 0x0000_0400;
        const MOVE_SELF = 0x0000_0800;
        const UNMOUNT = 0x0000_2000;
        const Q_OVERFLOW = 0x0000_4000;
        const IGNORED = 0x0000_8000;
        const ONLYDIR = 0x0100_0000;
        const DONT_FOLLOW = 0x0200_0000;
        const EXCL_UNLINK = 0x0400_0000;
        const MASK_CREATE = 0x1000_0000;
        const MASK_ADD = 0x2000_0000;
        const ISDIR = 0x4000_0000;
        const ONESHOT = 0x8000_0000;
        const ALL_EVENTS = Self::READ.bits()
            | Self::MODIFY.bits()
            | Self::ATTRIB.bits()
            | Self::CLOSE_WRITE.bits()
            | Self::CLOSE_NOWRITE.bits()
            | Self::OPEN.bits()
            | Self::MOVED_FROM.bits()
            | Self::MOVED_TO.bits()
            | Self::CREATE.bits()
            | Self::DELETE.bits()
            | Self::DELETE_SELF.bits()
            | Self::MOVE_SELF.bits();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InotifyRecord {
    pub mask: InotifyEvent,
    pub cookie: u32,
    pub name: String,
}

impl InotifyRecord {
    pub fn new(mask: InotifyEvent, cookie: u32, name: &str) -> Self {
        Self {
            mask,
            cookie,
            name: name.into(),
        }
    }
}

pub trait InotifyListener: Send + Sync {
    fn notify(&self, record: &InotifyRecord);
}

pub struct Notifier {
    events: SpinLock<InotifyEvent>,
    listeners: SpinLock<Vec<Weak<dyn InotifyListener>>>,
}

impl Notifier {
    pub fn new() -> Self {
        Self {
            events: SpinLock::new(InotifyEvent::empty(), "Notifier::events"),
            listeners: SpinLock::new(Vec::new(), "Notifier::listeners"),
        }
    }

    pub fn subscribe(&self, listener: &Arc<dyn InotifyListener>) {
        self.listeners.lock().push(Arc::downgrade(listener));
    }

    pub fn notify(&self, record: InotifyRecord) {
        self.events.lock().insert(record.mask);

        let listeners = {
            let mut listeners = self.listeners.lock();
            let mut live = Vec::new();
            listeners.retain(|listener| {
                if let Some(listener) = listener.upgrade() {
                    live.push(listener);
                    true
                } else {
                    false
                }
            });
            live
        };

        for listener in listeners {
            listener.notify(&record);
        }
    }
}
