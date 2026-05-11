use bitflags::bitflags;

use crate::fs::inode::Index;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FanotifyEventMask: u64 {
        const FAN_ACCESS = 0x0000_0001;
        const FAN_MODIFY = 0x0000_0002;
        const FAN_CLOSE_WRITE = 0x0000_0008;
        const FAN_CLOSE_NOWRITE = 0x0000_0010;
        const FAN_OPEN = 0x0000_0020;
        const FAN_DELETE = 0x0000_0200;
        const FAN_OPEN_EXEC = 0x0000_1000;
        const FAN_RENAME = 0x1000_0000;
        const FAN_OPEN_PERM = 0x0001_0000;
        const FAN_ACCESS_PERM = 0x0002_0000;
        const FAN_OPEN_EXEC_PERM = 0x0004_0000;
        const FAN_EVENT_ON_CHILD = 0x0800_0000;
        const FAN_ONDIR = 0x4000_0000;
    }
}

impl FanotifyEventMask {
    pub(super) fn event_bits(self) -> Self {
        self & (Self::FAN_ACCESS
            | Self::FAN_MODIFY
            | Self::FAN_CLOSE_WRITE
            | Self::FAN_CLOSE_NOWRITE
            | Self::FAN_OPEN
            | Self::FAN_DELETE
            | Self::FAN_OPEN_EXEC
            | Self::FAN_OPEN_PERM
            | Self::FAN_ACCESS_PERM
            | Self::FAN_OPEN_EXEC_PERM)
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FanotifyMarkFlags: usize {
        const FAN_MARK_ADD = 0x0000_0001;
        const FAN_MARK_REMOVE = 0x0000_0002;
        const FAN_MARK_DONT_FOLLOW = 0x0000_0004;
        const FAN_MARK_ONLYDIR = 0x0000_0008;
        const FAN_MARK_MOUNT = 0x0000_0010;
        const FAN_MARK_IGNORED_MASK = 0x0000_0020;
        const FAN_MARK_IGNORED_SURV_MODIFY = 0x0000_0040;
        const FAN_MARK_FLUSH = 0x0000_0080;
        const FAN_MARK_FILESYSTEM = 0x0000_0100;
        const FAN_MARK_EVICTABLE = 0x0000_0200;
        const FAN_MARK_IGNORE = 0x0000_0400;
    }
}

impl FanotifyMarkFlags {
    pub fn has_conflicting_scope_flags(self) -> bool {
        self.contains(Self::FAN_MARK_MOUNT) && self.contains(Self::FAN_MARK_FILESYSTEM)
    }

    pub fn has_conflicting_ignore_flags(self) -> bool {
        self.contains(Self::FAN_MARK_IGNORE) && self.contains(Self::FAN_MARK_IGNORED_MASK)
    }

    pub fn is_ignore_mark(self) -> bool {
        self.intersects(Self::FAN_MARK_IGNORE | Self::FAN_MARK_IGNORED_MASK)
    }

    pub fn is_ignore_without_surv_modify(self) -> bool {
        self.contains(Self::FAN_MARK_IGNORE) && !self.contains(Self::FAN_MARK_IGNORED_SURV_MODIFY)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FanotifyFdinfoKey {
    index: Option<Index>,
    mount_id: Option<usize>,
}

impl FanotifyFdinfoKey {
    pub(crate) fn new(index: Option<Index>, mount_id: Option<usize>) -> Self {
        Self { index, mount_id }
    }

    pub(super) fn index(self) -> Option<Index> {
        self.index
    }
}
