use alloc::sync::Arc;
use core::option::Option;

use crate::driver::BlockDriverOps;
use crate::fs::{InodeOps, Mode, VfsInode};
use crate::kernel::errno::{Errno, SysResult};
#[cfg(feature = "fanotify")]
use crate::kernel::event::Fanotify;
use crate::kernel::uapi::{Statfs, StatfsFlags};

use super::inode::Inode;

#[derive(Debug, Clone, Copy, Default)]
pub struct MountOptions {
    pub read_only: bool,
}

impl MountOptions {
    pub const fn new(read_only: bool) -> Self {
        Self { read_only }
    }
}

pub trait FileSystemOps: Send + Sync {
    fn create(
        &self,
        fsno: u32,
        driver: Option<Arc<dyn BlockDriverOps>>,
        options: MountOptions,
    ) -> SysResult<Arc<dyn VfsSuperBlockOps>>;
}

pub struct VfsSuperBlock<T: SuperBlockOps> {
    inner: T,
}

impl<T: SuperBlockOps> VfsSuperBlock<T> {
    pub fn new(inner: T) -> Arc<Self> {
        Arc::new(VfsSuperBlock { inner })
    }
}

pub trait SuperBlockOps: Send + Sync + 'static {
    type Inode: InodeOps;

    fn get_root_ino(&self) -> u32;

    fn get_inode(&self, ino: u32) -> SysResult<Self::Inode>;

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        None
    }

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify()
    }

    fn create_temp(&self, _mode: Mode) -> SysResult<Self::Inode> {
        Err(Errno::EOPNOTSUPP)
    }

    fn unmount(&self) -> SysResult<()> {
        // Default implementation does nothing,
        // can be overridden by specific filesystems
        Ok(())
    }

    fn statfs(&self) -> SysResult<Statfs> {
        Err(Errno::EOPNOTSUPP)
    }

    fn sync(&self) -> SysResult<()> {
        // Default implementation does nothing, can be overridden by specific filesystems
        Ok(())
    }

    fn is_readonly(&self) -> bool {
        false
    }

    fn statfs_flags(&self) -> StatfsFlags {
        if self.is_readonly() {
            StatfsFlags::ST_RDONLY
        } else {
            StatfsFlags::empty()
        }
    }

    fn type_name(&self) -> &'static str;
}

impl<T: SuperBlockOps> SuperBlockOps for Arc<T> {
    type Inode = T::Inode;

    fn get_root_ino(&self) -> u32 {
        self.as_ref().get_root_ino()
    }

    fn get_inode(&self, ino: u32) -> SysResult<Self::Inode> {
        self.as_ref().get_inode(ino)
    }

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        self.as_ref().fanotify()
    }

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        self.as_ref().ensure_fanotify()
    }

    fn create_temp(&self, mode: Mode) -> SysResult<Self::Inode> {
        self.as_ref().create_temp(mode)
    }

    fn unmount(&self) -> SysResult<()> {
        self.as_ref().unmount()
    }

    fn statfs(&self) -> SysResult<Statfs> {
        self.as_ref().statfs()
    }

    fn sync(&self) -> SysResult<()> {
        self.as_ref().sync()
    }

    fn is_readonly(&self) -> bool {
        self.as_ref().is_readonly()
    }

    fn statfs_flags(&self) -> StatfsFlags {
        self.as_ref().statfs_flags()
    }

    fn type_name(&self) -> &'static str {
        self.as_ref().type_name()
    }
}

pub trait VfsSuperBlockOps: Send + Sync {
    fn get_root_ino(&self) -> u32;

    fn get_inode(&self, ino: u32) -> SysResult<Arc<Inode>>;

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        None
    }

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify()
    }

    fn create_temp(&self, _mode: Mode) -> SysResult<Arc<Inode>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn unmount(&self) -> SysResult<()> {
        Ok(())
    }

    fn statfs(&self) -> SysResult<Statfs> {
        Err(Errno::EOPNOTSUPP)
    }

    fn sync(&self) -> SysResult<()> {
        Ok(())
    }

    fn is_readonly(&self) -> bool {
        false
    }

    fn statfs_flags(&self) -> StatfsFlags {
        if self.is_readonly() {
            StatfsFlags::ST_RDONLY
        } else {
            StatfsFlags::empty()
        }
    }

    fn type_name(&self) -> &'static str;
}

impl<T: SuperBlockOps> VfsSuperBlockOps for VfsSuperBlock<T> {
    fn get_root_ino(&self) -> u32 {
        self.inner.get_root_ino()
    }

    fn get_inode(&self, ino: u32) -> SysResult<Arc<Inode>> {
        Ok(VfsInode::new(self.inner.get_inode(ino)?))
    }

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        self.inner.fanotify()
    }

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        self.inner.ensure_fanotify()
    }

    fn create_temp(&self, mode: Mode) -> SysResult<Arc<Inode>> {
        Ok(VfsInode::new(self.inner.create_temp(mode)?))
    }

    fn unmount(&self) -> SysResult<()> {
        self.inner.unmount()
    }

    fn statfs(&self) -> SysResult<Statfs> {
        self.inner.statfs()
    }

    fn sync(&self) -> SysResult<()> {
        self.inner.sync()
    }

    fn is_readonly(&self) -> bool {
        self.inner.is_readonly()
    }

    fn statfs_flags(&self) -> StatfsFlags {
        self.inner.statfs_flags()
    }

    fn type_name(&self) -> &'static str {
        self.inner.type_name()
    }
}
