use alloc::string::String;
use alloc::sync::Arc;

use crate::fs::file::FileOps;
use crate::fs::inode::Index;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;
use crate::kernel::task::fdtable::FDFlags;

use super::super::types::FanotifyEventMask;
use super::inner::FanotifyListener;
use super::permission::FanotifyPermission;

#[repr(C)]
#[derive(Clone, Copy)]
struct FanotifyEventMetadata {
    event_len: u32,
    vers: u8,
    reserved: u8,
    metadata_len: u16,
    mask: u64,
    fd: i32,
    pid: i32,
}

impl FanotifyEventMetadata {
    const VERSION: u8 = 3;
    const SIZE: usize = core::mem::size_of::<Self>();

    fn new(mask: FanotifyEventMask, fd: i32, pid: i32, event_len: usize) -> Self {
        Self {
            event_len: event_len as u32,
            vers: Self::VERSION,
            reserved: 0,
            metadata_len: Self::SIZE as u16,
            mask: mask.bits(),
            fd,
            pid,
        }
    }

    fn write_to(&self, buf: &mut [u8]) {
        buf[0..4].copy_from_slice(&self.event_len.to_ne_bytes());
        buf[4] = self.vers;
        buf[5] = self.reserved;
        buf[6..8].copy_from_slice(&self.metadata_len.to_ne_bytes());
        buf[8..16].copy_from_slice(&self.mask.to_ne_bytes());
        buf[16..20].copy_from_slice(&self.fd.to_ne_bytes());
        buf[20..24].copy_from_slice(&self.pid.to_ne_bytes());
    }
}

struct FanotifyDfidNameInfo {
    parent_index: Index,
    name: String,
}

impl FanotifyDfidNameInfo {
    const EVENT_INFO_TYPE: u8 = 2;
    const EVENT_INFO_HEADER_SIZE: usize = 4;
    const FSID_SIZE: usize = 8;
    const FILE_HANDLE_HEADER_SIZE: usize = 8;
    const FILE_HANDLE_BYTES: usize = core::mem::size_of::<u32>() * 2;
    const FILE_HANDLE_TYPE_INODE: i32 = 1;

    fn from_file(file: Option<&Arc<dyn FileOps>>) -> Self {
        let dentry = file.and_then(|file| file.get_dentry());
        let parent = dentry.and_then(|dentry| dentry.get_parent());
        let parent_index = parent
            .as_ref()
            .map(|parent| parent.get_inode_index())
            .or_else(|| dentry.map(|dentry| dentry.get_inode_index()))
            .unwrap_or(Index { sno: 0, ino: 0 });
        let name = dentry.map(|dentry| dentry.name()).unwrap_or_default();

        Self { parent_index, name }
    }

    fn len(&self) -> usize {
        Self::EVENT_INFO_HEADER_SIZE
            + Self::FSID_SIZE
            + Self::FILE_HANDLE_HEADER_SIZE
            + Self::FILE_HANDLE_BYTES
            + self.name.len()
            + 1
    }

    fn write_to(&self, buf: &mut [u8]) {
        let len = self.len();
        let mut offset = 0;

        buf[offset] = Self::EVENT_INFO_TYPE;
        buf[offset + 1] = 0;
        buf[offset + 2..offset + 4].copy_from_slice(&(len as u16).to_ne_bytes());
        offset += Self::EVENT_INFO_HEADER_SIZE;

        buf[offset..offset + 4].copy_from_slice(&self.parent_index.sno.to_ne_bytes());
        buf[offset + 4..offset + 8].copy_from_slice(&0u32.to_ne_bytes());
        offset += Self::FSID_SIZE;

        buf[offset..offset + 4].copy_from_slice(&(Self::FILE_HANDLE_BYTES as u32).to_ne_bytes());
        buf[offset + 4..offset + 8].copy_from_slice(&Self::FILE_HANDLE_TYPE_INODE.to_ne_bytes());
        offset += Self::FILE_HANDLE_HEADER_SIZE;

        buf[offset..offset + 4].copy_from_slice(&self.parent_index.sno.to_ne_bytes());
        buf[offset + 4..offset + 8].copy_from_slice(&self.parent_index.ino.to_ne_bytes());
        offset += Self::FILE_HANDLE_BYTES;

        buf[offset..offset + self.name.len()].copy_from_slice(self.name.as_bytes());
    }
}

#[derive(Clone)]
pub(super) struct FanotifyEvent {
    pub(super) mask: FanotifyEventMask,
    pub(super) file: Option<Arc<dyn FileOps>>,
    pub(super) pid: i32,
    pub(super) permission: Option<Arc<FanotifyPermission>>,
}

impl FanotifyEvent {
    const NOFD: i32 = -1;
    pub(super) const MIN_READ_SIZE: usize = FanotifyEventMetadata::SIZE;

    fn align_len(len: usize) -> usize {
        (len + 7) & !7
    }

    pub(super) fn encoded_len(&self, inner: &FanotifyListener) -> usize {
        let info_len = if inner.report_dfid_name {
            FanotifyDfidNameInfo::from_file(self.file.as_ref()).len()
        } else {
            0
        };
        Self::align_len(FanotifyEventMetadata::SIZE + info_len)
    }

    pub(super) fn target_matches(&self, other: &Self) -> bool {
        match (self.file.as_ref(), other.file.as_ref()) {
            (Some(self_file), Some(other_file)) => match (self_file.get_dentry(), other_file.get_dentry()) {
                (Some(self_dentry), Some(other_dentry)) => {
                    self_dentry.get_inode_index() == other_dentry.get_inode_index()
                }
                _ => Arc::ptr_eq(self_file, other_file),
            },
            _ => false,
        }
    }

    pub(super) fn merges_with(&self, other: &Self) -> bool {
        self.permission.is_none() && other.permission.is_none() && self.pid == other.pid && self.target_matches(other)
    }

    pub(super) fn merge(&mut self, other: &Self) {
        self.mask.insert(other.mask);
    }

    pub(super) fn write_to(mut self, inner: &FanotifyListener, buf: &mut [u8]) -> SysResult<usize> {
        let event_len = self.encoded_len(inner);
        if buf.len() < event_len {
            return Err(Errno::EINVAL);
        }

        for byte in &mut buf[..event_len] {
            *byte = 0;
        }

        let pid = if inner.unprivileged && current::pid() as i32 != self.pid {
            0
        } else {
            self.pid
        };
        let fd = if inner.unprivileged || (inner.report_dfid_name && self.permission.is_none()) {
            Self::NOFD
        } else if let Some(file) = self.file.take() {
            let fd = current::fdtable().lock().push(file, FDFlags::empty())? as i32;
            if let Some(permission) = self.permission {
                inner.responses.lock().push((fd, permission));
            }
            fd
        } else {
            Self::NOFD
        };

        FanotifyEventMetadata::new(self.mask, fd, pid, event_len).write_to(&mut buf[..FanotifyEventMetadata::SIZE]);
        if inner.report_dfid_name {
            FanotifyDfidNameInfo::from_file(self.file.as_ref())
                .write_to(&mut buf[FanotifyEventMetadata::SIZE..event_len]);
        }

        Ok(event_len)
    }
}
