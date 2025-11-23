use core::{marker::PhantomData, mem, time::Duration};

use alloc::boxed::Box;

use crate::{
    DirLookupResult, DirReader, Ext4Error, Ext4Result, FileAttr, InodeRef, InodeType,
    blockdev::{BlockDevice, Ext4BlockDevice},
    error::Context,
    ffi::*,
    util::get_block_size,
};

pub trait SystemHal {
    fn now() -> Option<Duration>;
}

pub struct DummyHal;
impl SystemHal for DummyHal {
    fn now() -> Option<Duration> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct FsConfig {
    pub bcache_size: u32,
}
impl Default for FsConfig {
    fn default() -> Self {
        Self {
            bcache_size: CONFIG_BLOCK_DEV_CACHE_SIZE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StatFs {
    pub inodes_count: u32,
    pub free_inodes_count: u32,

    pub blocks_count: u64,
    pub free_blocks_count: u64,
    pub block_size: u32,
}

pub struct Ext4Filesystem<Hal: SystemHal, Dev: BlockDevice> {
    inner: Box<ext4_fs>,
    bdev: Ext4BlockDevice<Dev>,
    _phantom: PhantomData<Hal>,
}

impl<Hal: SystemHal, Dev: BlockDevice> Ext4Filesystem<Hal, Dev> {
    pub fn new(dev: Dev, config: FsConfig) -> Ext4Result<Self> {
        let mut bdev = Ext4BlockDevice::new(dev)?;
        let mut fs = Box::new(unsafe { mem::zeroed() });
        unsafe {
            let bd = bdev.inner.as_mut();
            ext4_fs_init(&mut *fs, bd, false).context("ext4_fs_init")?;

            let bs = get_block_size(&fs.sb);
            ext4_block_set_lb_size(bd, bs);
            ext4_bcache_init_dynamic(bd.bc, config.bcache_size, bs)
                .context("ext4_bcache_init_dynamic")?;
            if bs != (*bd.bc).itemsize {
                return Err(Ext4Error::new(ENOTSUP as _, "block size mismatch"));
            }

            bd.fs = &mut *fs;

            let mut result = Self {
                inner: fs,
                bdev,
                _phantom: PhantomData,
            };
            let bd = result.bdev.inner.as_mut();
            ext4_block_bind_bcache(bd, bd.bc).context("ext4_block_bind_bcache")?;
            Ok(result)
        }
    }

    fn inode_ref(&mut self, ino: u32) -> Ext4Result<InodeRef<Hal>> {
        unsafe {
            let mut result = InodeRef::new(mem::zeroed());
            ext4_fs_get_inode_ref(self.inner.as_mut(), ino, result.inner.as_mut())
                .context("ext4_fs_get_inode_ref")?;
            Ok(result)
        }
    }
    fn clone_ref(&mut self, inode: &InodeRef<Hal>) -> InodeRef<Hal> {
        self.inode_ref(inode.ino()).expect("inode ref clone failed")
    }

    pub fn with_inode_ref<R>(
        &mut self,
        ino: u32,
        f: impl FnOnce(&mut InodeRef<Hal>) -> Ext4Result<R>,
    ) -> Ext4Result<R> {
        let mut inode = self.inode_ref(ino)?;
        f(&mut inode)
    }

    pub(crate) fn alloc_inode(&mut self, ty: InodeType) -> Ext4Result<InodeRef<Hal>> {
        unsafe {
            let ty = match ty {
                InodeType::Fifo => EXT4_DE_FIFO,
                InodeType::CharacterDevice => EXT4_DE_CHRDEV,
                InodeType::Directory => EXT4_DE_DIR,
                InodeType::BlockDevice => EXT4_DE_BLKDEV,
                InodeType::RegularFile => EXT4_DE_REG_FILE,
                InodeType::Symlink => EXT4_DE_SYMLINK,
                InodeType::Socket => EXT4_DE_SOCK,
                InodeType::Unknown => EXT4_DE_UNKNOWN,
            };
            let mut result = InodeRef::new(mem::zeroed());
            ext4_fs_alloc_inode(self.inner.as_mut(), result.inner.as_mut(), ty as _)
                .context("ext4_fs_get_inode_ref")?;
            ext4_fs_inode_blocks_init(self.inner.as_mut(), result.inner.as_mut());
            Ok(result)
        }
    }

    pub fn get_attr(&mut self, ino: u32, attr: &mut FileAttr) -> Ext4Result<()> {
        self.inode_ref(ino)?.get_attr(attr);
        Ok(())
    }

    pub fn read_at(&mut self, ino: u32, buf: &mut [u8], offset: u64) -> Ext4Result<usize> {
        self.inode_ref(ino)?.read_at(buf, offset)
    }
    pub fn write_at(&mut self, ino: u32, buf: &[u8], offset: u64) -> Ext4Result<usize> {
        self.inode_ref(ino)?.write_at(buf, offset)
    }
    pub fn set_len(&mut self, ino: u32, len: u64) -> Ext4Result<()> {
        self.inode_ref(ino)?.set_len(len)
    }
    pub fn set_symlink(&mut self, ino: u32, buf: &[u8]) -> Ext4Result<()> {
        self.inode_ref(ino)?.set_symlink(buf)
    }
    pub fn lookup(&mut self, parent: u32, name: &str) -> Ext4Result<DirLookupResult<Hal>> {
        self.inode_ref(parent)?.lookup(name)
    }
    pub fn read_dir(&mut self, parent: u32, offset: u64) -> Ext4Result<DirReader<Hal>> {
        self.inode_ref(parent)?.read_dir(offset)
    }

    pub fn create(&mut self, parent: u32, name: &str, ty: InodeType, mode: u32) -> Ext4Result<u32> {
        let mut child = self.alloc_inode(ty)?;
        let mut parent = self.inode_ref(parent)?;
        parent.add_entry(name, &mut child)?;
        if ty == InodeType::Directory {
            child.add_entry(".", &mut self.clone_ref(&child))?;
            child.add_entry("..", &mut parent)?;
            assert_eq!(child.nlink(), 2);
        }
        child.set_mode((child.mode() & !0o777) | (mode & 0o777));

        Ok(child.ino())
    }

    pub fn rename(
        &mut self,
        src_dir: u32,
        src_name: &str,
        dst_dir: u32,
        dst_name: &str,
    ) -> Ext4Result {
        let mut src_dir_ref = self.inode_ref(src_dir)?;
        let mut dst_dir_ref = self.inode_ref(dst_dir)?;

        // TODO: optimize
        match self.unlink(dst_dir, dst_name) {
            Ok(_) => {}
            Err(err) if err.code == ENOENT as i32 => {}
            Err(err) => return Err(err),
        }

        let src = self.lookup(src_dir, src_name)?.entry().ino();

        let mut src_ref = self.inode_ref(src)?;
        if src_ref.is_dir() {
            let mut result = self.clone_ref(&src_ref).lookup("..")?;
            result.entry().raw_entry_mut().set_ino(dst_dir);
            src_dir_ref.dec_nlink();
            dst_dir_ref.inc_nlink();
        }
        src_dir_ref.remove_entry(src_name, &mut src_ref)?;
        dst_dir_ref.add_entry(dst_name, &mut src_ref)?;

        Ok(())
    }

    pub fn link(&mut self, dir: u32, name: &str, child: u32) -> Ext4Result {
        let mut child_ref = self.inode_ref(child)?;
        if child_ref.is_dir() {
            return Err(Ext4Error::new(EISDIR as _, "cannot link to directory"));
        }
        self.inode_ref(dir)?.add_entry(name, &mut child_ref)?;
        Ok(())
    }

    pub fn unlink(&mut self, dir: u32, name: &str) -> Ext4Result {
        let mut dir_ref = self.inode_ref(dir)?;
        let child = self.clone_ref(&dir_ref).lookup(name)?.entry().ino();
        let mut child_ref = self.inode_ref(child)?;

        if self.clone_ref(&child_ref).has_children()? {
            return Err(Ext4Error::new(ENOTEMPTY as _, None));
        }
        if child_ref.inode_type() == InodeType::Directory {
            // According to `ext4_trunc_dir`
            let bs = get_block_size(&self.inner.as_mut().sb);
            child_ref.truncate(bs as _)?;
        }

        dir_ref.remove_entry(name, &mut child_ref)?;

        if child_ref.is_dir() {
            dir_ref.dec_nlink();
            child_ref.dec_nlink();
        }
        if child_ref.nlink() == 0 {
            child_ref.truncate(0)?;
            unsafe {
                ext4_inode_set_del_time(child_ref.inner.inode, u32::MAX);
                child_ref.mark_dirty();
                ext4_fs_free_inode(child_ref.inner.as_mut());
            }
        }
        Ok(())
    }

    pub fn stat(&mut self) -> Ext4Result<StatFs> {
        let sb = &mut self.inner.as_mut().sb;
        Ok(StatFs {
            inodes_count: u32::from_le(sb.inodes_count),
            free_inodes_count: u32::from_le(sb.free_inodes_count),
            blocks_count: (u32::from_le(sb.blocks_count_hi) as u64) << 32
                | u32::from_le(sb.blocks_count_lo) as u64,
            free_blocks_count: (u32::from_le(sb.free_blocks_count_hi) as u64) << 32
                | u32::from_le(sb.free_blocks_count_lo) as u64,
            block_size: get_block_size(sb),
        })
    }

    pub fn flush(&mut self) -> Ext4Result<()> {
        unsafe {
            ext4_block_cache_flush(self.bdev.inner.as_mut()).context("ext4_cache_flush")?;
        }
        Ok(())
    }
}

impl<Hal: SystemHal, Dev: BlockDevice> Drop for Ext4Filesystem<Hal, Dev> {
    fn drop(&mut self) {
        unsafe {
            let r = ext4_fs_fini(self.inner.as_mut());
            if r != 0 {
                log::error!("ext4_fs_fini failed: {}", Ext4Error::new(r, None));
            }
            let bdev = self.bdev.inner.as_mut();
            ext4_bcache_cleanup(bdev.bc);
            ext4_block_fini(bdev);
            ext4_bcache_fini_dynamic(bdev.bc);
        }
    }
}

// The underlying C structures contain raw pointers and are not automatically
// `Send`/`Sync`. In this project the filesystem is always synchronized by
// the surrounding kernel locking; mark the type as `Send`/`Sync` so it can
// be stored inside our kernel `Mutex`/`Arc`. This is an unsafe, but
// controlled decision.
unsafe impl<Hal: SystemHal, Dev: BlockDevice> Send for Ext4Filesystem<Hal, Dev> {}
unsafe impl<Hal: SystemHal, Dev: BlockDevice> Sync for Ext4Filesystem<Hal, Dev> {}

pub(crate) struct WritebackGuard {
    bdev: *mut ext4_blockdev,
}
impl WritebackGuard {
    pub fn new(bdev: *mut ext4_blockdev) -> Self {
        unsafe { ext4_block_cache_write_back(bdev, 1) };
        Self { bdev }
    }
}
impl Drop for WritebackGuard {
    fn drop(&mut self) {
        unsafe { ext4_block_cache_write_back(self.bdev, 0) };
    }
}
