use core::time::Duration;

use crate::{SystemHal, ffi::*, util::get_block_size};

use super::{InodeRef, InodeType};

/// Filesystem node metadata.
#[derive(Clone, Debug, Default)]
pub struct FileAttr {
    /// ID of device containing file
    pub device: u64,
    /// Inode number
    pub ino: u32,
    /// Number of hard links
    pub nlink: u64,
    /// Permission mode
    pub mode: u32,
    /// Type of file
    pub node_type: InodeType,
    /// User ID of owner
    pub uid: u32,
    /// Group ID of owner
    pub gid: u32,
    /// Total size in bytes
    pub size: u64,
    /// Block size for filesystem I/O
    pub block_size: u64,
    /// Number of 512B blocks allocated
    pub blocks: u64,

    /// Time of last access
    pub atime: Duration,
    /// Time of last modification
    pub mtime: Duration,
    /// Time of last status change
    pub ctime: Duration,
}

fn encode_time(dur: &Duration) -> (u32, u32) {
    let sec = dur.as_secs();
    let nsec = dur.subsec_nanos();
    let time = u32::to_le(sec as u32);
    let extra = u32::to_le((nsec << 2) | (sec >> 32) as u32);
    (time, extra)
}
fn decode_time(time: u32, extra: u32) -> Duration {
    let sec = u32::from_le(time);
    let extra = u32::from_le(extra);
    let epoch = extra & 3;
    let nsec = extra >> 2;

    Duration::new(sec as u64 + ((epoch as u64) << 32), nsec)
}

impl<Hal: SystemHal> InodeRef<Hal> {
    pub fn inode_type(&self) -> InodeType {
        ((self.mode() >> 12) as u8).into()
    }

    pub fn is_dir(&self) -> bool {
        self.inode_type() == InodeType::Directory
    }

    pub fn size(&self) -> u64 {
        unsafe { ext4_inode_get_size(self.superblock() as *const _ as _, self.inner.inode) }
    }

    pub fn mode(&self) -> u32 {
        unsafe { ext4_inode_get_mode(self.superblock() as *const _ as _, self.inner.inode) }
    }
    pub fn set_mode(&mut self, mode: u32) {
        unsafe {
            ext4_inode_set_mode(self.superblock_mut(), self.inner.inode, mode);
            self.mark_dirty();
        }
    }

    pub fn nlink(&self) -> u16 {
        u16::from_le(self.raw_inode().links_count)
    }

    pub fn uid(&self) -> u16 {
        u16::from_le(self.raw_inode().uid)
    }
    pub fn gid(&self) -> u16 {
        u16::from_le(self.raw_inode().gid)
    }

    pub fn set_owner(&mut self, uid: u16, gid: u16) {
        let inode = self.raw_inode_mut();
        inode.uid = u16::to_le(uid);
        inode.gid = u16::to_le(gid);
        self.mark_dirty();
    }

    pub fn set_atime(&mut self, dur: &Duration) {
        let (time, extra) = encode_time(dur);
        let inode = self.raw_inode_mut();
        inode.access_time = time;
        inode.atime_extra = extra;
        self.mark_dirty();
    }
    pub fn set_mtime(&mut self, dur: &Duration) {
        let (time, extra) = encode_time(dur);
        let inode = self.raw_inode_mut();
        inode.modification_time = time;
        inode.mtime_extra = extra;
        self.mark_dirty();
    }
    pub fn set_ctime(&mut self, dur: &Duration) {
        let (time, extra) = encode_time(dur);
        let inode = self.raw_inode_mut();
        inode.change_inode_time = time;
        inode.ctime_extra = extra;
        self.mark_dirty();
    }

    pub fn update_atime(&mut self) {
        if let Some(dur) = Hal::now() {
            self.set_atime(&dur);
        }
    }
    pub fn update_mtime(&mut self) {
        if let Some(dur) = Hal::now() {
            self.set_mtime(&dur);
        }
    }
    pub fn update_ctime(&mut self) {
        if let Some(dur) = Hal::now() {
            self.set_ctime(&dur);
        }
    }

    pub fn get_attr(&self, attr: &mut FileAttr) {
        attr.device = 0;
        attr.ino = u32::from_le(self.inner.index);
        attr.nlink = self.nlink() as _;
        attr.mode = self.mode();
        attr.node_type = self.inode_type();
        attr.uid = self.uid() as _;
        attr.gid = self.gid() as _;
        attr.size = self.size();
        attr.block_size = get_block_size(self.superblock()) as _;
        attr.blocks = unsafe {
            ext4_inode_get_blocks_count(self.superblock() as *const _ as _, self.inner.inode)
        };

        let inode = self.raw_inode();
        attr.atime = decode_time(inode.access_time, inode.atime_extra);
        attr.mtime = decode_time(inode.modification_time, inode.mtime_extra);
        attr.ctime = decode_time(inode.change_inode_time, inode.ctime_extra);
    }
}
