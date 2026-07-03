use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::time::Duration;

use crate::fs::inode::Mode;
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::uapi::Uid;

pub(super) struct FileMeta {
    pub(super) pages: BTreeMap<usize, Arc<PhysPageFrame>>,
    pub(super) filesize: usize,
}

impl FileMeta {
    fn new() -> Self {
        Self {
            pages: BTreeMap::new(),
            filesize: 0,
        }
    }
}

pub(super) enum Meta {
    File(FileMeta),
    Directory(BTreeMap<String, u32>),
    Symlink(String),
}

pub struct InodeMeta {
    pub(super) meta: Meta,
    pub(super) mode: Mode,
    pub(in crate::fs::memtreefs) owner: (Uid, Uid),
    pub(super) mtime: Duration,
    pub(super) atime: Duration,
    pub(super) ctime: Duration,
    pub(super) rdev: u64,
    pub(super) links: u32,
}

impl InodeMeta {
    pub fn new(mode: Mode, ino: u32, parent_ino: u32) -> Self {
        let meta = match mode & Mode::S_IFMT {
            Mode::S_IFDIR => {
                let mut children = BTreeMap::new();
                children.insert(".".into(), ino);
                children.insert("..".into(), parent_ino);
                Meta::Directory(children)
            }
            Mode::S_IFLNK => Meta::Symlink(String::new()),
            _ => Meta::File(FileMeta::new()),
        };
        Self {
            meta,
            mode,
            owner: (0, 0),
            mtime: Duration::ZERO,
            atime: Duration::ZERO,
            ctime: Duration::ZERO,
            rdev: 0,
            links: 0,
        }
    }
}
