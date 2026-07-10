use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;

use crate::driver::BlockDriverOps;
use crate::fs::ext4::blockdev::Ext4BlockDevice;
use crate::fs::ext4::ffi::*;
use crate::fs::ext4::inode::Ext4Inode;
use crate::fs::ext4::util::get_block_size;
use crate::fs::filesystem::SuperBlockOps;
use crate::kernel::errno::{Errno, SysResult};
#[cfg(feature = "fanotify")]
use crate::kernel::event::Fanotify;
use crate::kernel::uapi::{Statfs, StatfsFlags};
#[cfg(feature = "fanotify")]
use crate::klib::LazyInitedCell;
use crate::klib::SleepLock;
use crate::kwarn;

pub(super) fn map_error_to_kernel(code: i32) -> Errno {
    Errno::try_from(code).unwrap_or(Errno::EIO)
}

fn ext4_result(code: i32) -> SysResult<()> {
    if code == EOK as i32 {
        Ok(())
    } else {
        Err(map_error_to_kernel(code))
    }
}

struct CachedInodeRef {
    ino: u32,
    inode_ref: ext4_inode_ref,
    active_count: usize,
}

impl CachedInodeRef {
    fn matches(&self, inode_ref: &ext4_inode_ref) -> bool {
        self.ino == inode_ref.index
            && self.inode_ref.inode == inode_ref.inode
            && self.inode_ref.block.buf == inode_ref.block.buf
    }
}

pub(super) struct SuperBlockInner {
    pub(super) fs: Box<ext4_fs>,
    bdev: Ext4BlockDevice,
    inode_ref_cache: Vec<CachedInodeRef>,
}

impl SuperBlockInner {
    const INODE_REF_CACHE_SIZE: usize = 16;

    fn new(driver: Arc<dyn BlockDriverOps>, read_only: bool) -> SysResult<Self> {
        let mut bdev = Ext4BlockDevice::new(driver).map_err(map_error_to_kernel)?;
        let mut fs: Box<ext4_fs> = Box::new(unsafe { mem::zeroed() });

        unsafe {
            let init_rc = ext4_fs_init(fs.as_mut(), bdev.inner.as_mut(), read_only);
            if init_rc != EOK as i32 {
                kwarn!(
                    "ext4_fs_init failed rc={} magic={:#x} rev={} compat={:#x} incompat={:#x} ro_compat={:#x} log_block_size={} inode_size={} desc_size={}",
                    init_rc,
                    u16::from_le(fs.sb.magic),
                    u32::from_le(fs.sb.rev_level),
                    u32::from_le(fs.sb.features_compatible),
                    u32::from_le(fs.sb.features_incompatible),
                    u32::from_le(fs.sb.features_read_only),
                    u32::from_le(fs.sb.log_block_size),
                    u16::from_le(fs.sb.inode_size),
                    u16::from_le(fs.sb.desc_size),
                );
            }
            ext4_result(init_rc)?;

            let block_size = get_block_size(&fs.sb);
            ext4_block_set_lb_size(bdev.inner.as_mut(), block_size);
            let bcache_rc = ext4_bcache_init_dynamic(bdev.inner.as_mut().bc, CONFIG_BLOCK_DEV_CACHE_SIZE, block_size);
            if bcache_rc != EOK as i32 {
                kwarn!(
                    "ext4_bcache_init_dynamic failed rc={} block_size={} cache_size={}",
                    bcache_rc,
                    block_size,
                    CONFIG_BLOCK_DEV_CACHE_SIZE
                );
            }
            ext4_result(bcache_rc)?;

            let cache_itemsize = (*bdev.inner.as_mut().bc).itemsize;
            if block_size != cache_itemsize {
                kwarn!(
                    "ext4 block cache itemsize mismatch block_size={} cache_itemsize={}",
                    block_size,
                    cache_itemsize
                );
                return Err(Errno::EOPNOTSUPP);
            }

            bdev.inner.as_mut().fs = fs.as_mut();
            let bind_rc = ext4_block_bind_bcache(bdev.inner.as_mut(), bdev.inner.as_mut().bc);
            if bind_rc != EOK as i32 {
                kwarn!("ext4_block_bind_bcache failed rc={}", bind_rc);
            }
            ext4_result(bind_rc)?;
        }

        Ok(Self {
            fs,
            bdev,
            inode_ref_cache: Vec::new(),
        })
    }

    fn read_inode_ref_uncached(&mut self, ino: u32) -> SysResult<ext4_inode_ref> {
        let mut result: ext4_inode_ref = unsafe { mem::zeroed() };
        unsafe {
            ext4_result(kernelx_ext4_read_inode_ref(self.fs.as_mut(), ino, &mut result))?;
        }
        Ok(result)
    }

    fn put_inode_ref_uncached(inode_ref: &mut ext4_inode_ref) -> SysResult<()> {
        unsafe { ext4_result(ext4_fs_put_inode_ref(inode_ref)) }
    }

    fn cached_inode_ref_position(&self, ino: u32) -> Option<usize> {
        self.inode_ref_cache.iter().position(|cached| cached.ino == ino)
    }

    fn evict_cached_inode_ref(&mut self, position: usize) -> SysResult<()> {
        let mut cached = self.inode_ref_cache.remove(position);
        debug_assert_eq!(cached.active_count, 0);
        Self::put_inode_ref_uncached(&mut cached.inode_ref)
    }

    fn cache_inode_ref(&mut self, inode_ref: ext4_inode_ref) -> SysResult<bool> {
        if self.cached_inode_ref_position(inode_ref.index).is_some() {
            return Ok(true);
        }

        if self.inode_ref_cache.len() >= Self::INODE_REF_CACHE_SIZE {
            let Some(position) = self.inode_ref_cache.iter().position(|cached| cached.active_count == 0) else {
                return Ok(false);
            };
            self.evict_cached_inode_ref(position)?;
        }

        self.inode_ref_cache.push(CachedInodeRef {
            ino: inode_ref.index,
            inode_ref,
            active_count: 1,
        });
        Ok(true)
    }

    fn drop_inode_ref_cache(&mut self) -> SysResult<()> {
        let mut first_err = None;
        while let Some(mut cached) = self.inode_ref_cache.pop() {
            if let Err(err) = Self::put_inode_ref_uncached(&mut cached.inode_ref) {
                first_err.get_or_insert(err);
            }
        }

        if let Some(err) = first_err { Err(err) } else { Ok(()) }
    }

    pub(crate) fn read_inode_ref(&mut self, ino: u32) -> SysResult<ext4_inode_ref> {
        if let Some(position) = self.cached_inode_ref_position(ino) {
            let mut cached = self.inode_ref_cache.remove(position);
            cached.active_count += 1;
            let inode_ref = cached.inode_ref;
            self.inode_ref_cache.push(cached);
            return Ok(inode_ref);
        }

        let mut result = self.read_inode_ref_uncached(ino)?;
        match self.cache_inode_ref(result) {
            Ok(true) => {}
            Ok(false) => return Ok(result),
            Err(err) => {
                let _ = Self::put_inode_ref_uncached(&mut result);
                return Err(err);
            }
        }
        Ok(result)
    }

    pub(crate) fn put_inode_ref(&mut self, inode_ref: &mut ext4_inode_ref) -> SysResult<()> {
        if let Some(cached) = self.inode_ref_cache.iter_mut().find(|cached| cached.matches(inode_ref)) {
            debug_assert!(cached.active_count > 0);
            if cached.active_count > 0 {
                cached.active_count -= 1;
            }
            cached.inode_ref.dirty |= inode_ref.dirty;
            inode_ref.dirty = false;
            return Ok(());
        }

        Self::put_inode_ref_uncached(inode_ref)
    }

    pub(crate) fn invalidate_inode_ref(&mut self, ino: u32) -> SysResult<()> {
        if let Some(position) = self.cached_inode_ref_position(ino) {
            self.evict_cached_inode_ref(position)?;
        }
        Ok(())
    }

    pub(crate) fn alloc_inode(&mut self, filetype: i32) -> SysResult<ext4_inode_ref> {
        let mut result: ext4_inode_ref = unsafe { mem::zeroed() };
        unsafe {
            ext4_result(ext4_fs_alloc_inode(self.fs.as_mut(), &mut result, filetype))?;
            ext4_fs_inode_blocks_init(self.fs.as_mut(), &mut result);
        }
        Ok(result)
    }

    pub(crate) fn flush(&mut self) -> SysResult<()> {
        self.drop_inode_ref_cache()?;
        unsafe {
            ext4_result(ext4_block_cache_flush(self.bdev.inner.as_mut()))?;
            if !self.fs.read_only {
                ext4_result(ext4_sb_write(self.bdev.inner.as_mut(), &mut self.fs.sb))?;
            }
        }
        self.bdev.flush_driver().map_err(|_| Errno::EIO)?;
        Ok(())
    }
}

impl Drop for SuperBlockInner {
    fn drop(&mut self) {
        let _ = self.drop_inode_ref_cache();
        unsafe {
            let rc = ext4_fs_fini(self.fs.as_mut());
            let _ = rc;
            let _ = self.bdev.flush_driver();

            ext4_bcache_cleanup(self.bdev.inner.as_mut().bc);
            ext4_block_fini(self.bdev.inner.as_mut());
            let rc = ext4_bcache_fini_dynamic(self.bdev.inner.as_mut().bc);
            let _ = rc;
        }
    }
}

unsafe impl Send for SuperBlockInner {}
unsafe impl Sync for SuperBlockInner {}

pub struct Ext4SuperBlock {
    superblock: Arc<SleepLock<SuperBlockInner>>,
    #[cfg(feature = "fanotify")]
    fanotify: LazyInitedCell<Arc<Fanotify>>,
}

impl Ext4SuperBlock {
    pub fn new(driver: Arc<dyn BlockDriverOps>, read_only: bool) -> SysResult<Arc<Self>> {
        Ok(Arc::new(Self {
            superblock: Arc::new(SleepLock::new(
                SuperBlockInner::new(driver, read_only)?,
                "Ext4SuperBlock::superblock",
            )),
            #[cfg(feature = "fanotify")]
            fanotify: LazyInitedCell::new("Ext4SuperBlock::fanotify"),
        }))
    }

    pub(crate) fn inner(&self) -> Arc<SleepLock<SuperBlockInner>> {
        self.superblock.clone()
    }
}

impl SuperBlockOps for Ext4SuperBlock {
    type Inode = Ext4Inode;

    fn get_inode(&self, ino: u32) -> SysResult<Self::Inode> {
        Ext4Inode::new(ino, self.inner())
    }

    fn get_root_ino(&self) -> u32 {
        2
    }

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify.get()
    }

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        Some(self.fanotify.get_or_init(|| Arc::new(Fanotify::new())))
    }

    fn unmount(&self) -> SysResult<()> {
        let mut superblock = self.superblock.lock();
        superblock.flush()?;
        unsafe { ext4_result(ext4_fs_fini(superblock.fs.as_mut())) }
    }

    fn statfs(&self) -> SysResult<Statfs> {
        let superblock = self.superblock.lock();
        let sb = &superblock.fs.sb;
        Ok(Statfs {
            f_type: 0xEF53,
            f_bsize: get_block_size(sb) as u64,
            f_blocks: ((u32::from_le(sb.blocks_count_hi) as u64) << 32) | u32::from_le(sb.blocks_count_lo) as u64,
            f_bfree: ((u32::from_le(sb.free_blocks_count_hi) as u64) << 32)
                | u32::from_le(sb.free_blocks_count_lo) as u64,
            f_bavail: ((u32::from_le(sb.free_blocks_count_hi) as u64) << 32)
                | u32::from_le(sb.free_blocks_count_lo) as u64,
            f_files: u32::from_le(sb.inodes_count) as u64,
            f_ffree: u32::from_le(sb.free_inodes_count) as u64,
            f_fsid: 0,
            f_namelen: 255,
            f_frsize: get_block_size(sb) as u64,
            f_flag: if superblock.fs.read_only {
                StatfsFlags::ST_RDONLY
            } else {
                StatfsFlags::empty()
            }
            .bits(),
            f_spare: [0; 4],
        })
    }

    fn sync(&self) -> SysResult<()> {
        self.superblock.lock().flush()
    }

    fn is_readonly(&self) -> bool {
        self.superblock.lock().fs.read_only
    }

    fn type_name(&self) -> &'static str {
        "ext4"
    }
}
