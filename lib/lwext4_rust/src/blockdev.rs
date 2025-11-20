use core::{
    ffi::{c_int, c_void},
    mem, ptr, slice,
};

use crate::{Ext4Result, error::Context, ffi::*};
use alloc::boxed::Box;

/// Device block size.
pub const EXT4_DEV_BSIZE: usize = 512;

pub trait BlockDevice {
    /// Writes blocks to the device, starting from the given block ID.
    fn write_blocks(&mut self, block_id: u64, buf: &[u8]) -> Ext4Result<usize>;

    /// Reads blocks from the device, starting from the given block ID.
    fn read_blocks(&mut self, block_id: u64, buf: &mut [u8]) -> Ext4Result<usize>;

    /// Gets the number of blocks on the device.
    fn num_blocks(&self) -> Ext4Result<u64>;
}

/// Holds necessary resources for the ext4 block device, and automatically frees
/// them when the instance is dropped.
#[allow(dead_code)]
struct ResourceGuard<Dev> {
    dev: Box<Dev>,
    block_buf: Box<[u8; EXT4_DEV_BSIZE]>,
    block_cache_buf: Box<ext4_bcache>,
    block_dev_iface: Box<ext4_blockdev_iface>,
}

pub struct Ext4BlockDevice<Dev: BlockDevice> {
    pub(crate) inner: Box<ext4_blockdev>,
    _guard: ResourceGuard<Dev>,
}

impl<Dev: BlockDevice> Ext4BlockDevice<Dev> {
    pub fn new(dev: Dev) -> Ext4Result<Self> {
        let mut dev = Box::new(dev);

        // Block size buffer
        let mut block_buf = Box::new([0u8; EXT4_DEV_BSIZE]);
        let mut block_dev_iface = Box::new(ext4_blockdev_iface {
            open: Some(Self::dev_open),
            bread: Some(Self::dev_bread),
            bwrite: Some(Self::dev_bwrite),
            close: Some(Self::dev_close),
            lock: None,
            unlock: None,
            ph_bsize: EXT4_DEV_BSIZE as u32,
            ph_bcnt: 0,
            ph_bbuf: block_buf.as_mut_ptr(),
            ph_refctr: 0,
            bread_ctr: 0,
            bwrite_ctr: 0,
            p_user: dev.as_mut() as *mut _ as *mut c_void,
        });

        let mut block_cache_buf: Box<ext4_bcache> = Box::new(unsafe { mem::zeroed() });
        let mut blockdev = Box::new(ext4_blockdev {
            bdif: block_dev_iface.as_mut(),
            part_offset: 0,
            part_size: 0,
            bc: block_cache_buf.as_mut(),
            lg_bsize: 0,
            lg_bcnt: 0,
            cache_write_back: 0,
            fs: ptr::null_mut(),
            journal: ptr::null_mut(),
        });

        unsafe {
            ext4_block_init(blockdev.as_mut()).context("ext4_block_init")?;
            ext4_block_cache_write_back(blockdev.as_mut(), 1)
                .context("ext4_block_cache_write_back")
                .inspect_err(|_| {
                    ext4_block_fini(blockdev.as_mut());
                })?;
        }
        Ok(Self {
            inner: blockdev,
            _guard: ResourceGuard {
                dev,
                block_buf,
                block_cache_buf,
                block_dev_iface,
            },
        })
    }

    unsafe fn dev_read_fields<'a>(
        bdev: *mut ext4_blockdev,
    ) -> (
        &'a mut ext4_blockdev,
        &'a mut ext4_blockdev_iface,
        &'a mut Dev,
    ) {
        let bdev = unsafe { &mut *bdev };
        let bdif = unsafe { &mut *bdev.bdif };
        let dev = unsafe { &mut *(bdif.p_user as *mut Dev) };
        (bdev, bdif, dev)
    }
    unsafe extern "C" fn dev_open(bdev: *mut ext4_blockdev) -> c_int {
        debug!("open ext4 block device");
        let (bdev, bdif, dev) = unsafe { Self::dev_read_fields(bdev) };

        bdif.ph_bcnt = match dev.num_blocks() {
            Ok(cur) => cur,
            Err(err) => {
                error!("num_blocks failed: {err:?}");
                return EIO as _;
            }
        };

        bdev.part_offset = 0;
        bdev.part_size = bdif.ph_bcnt * bdif.ph_bsize as u64;
        EOK as _
    }
    unsafe extern "C" fn dev_bread(
        bdev: *mut ext4_blockdev,
        buf: *mut c_void,
        blk_id: u64,
        blk_cnt: u32,
    ) -> c_int {
        trace!("read ext4 block id={blk_id} count={blk_cnt}");
        if blk_cnt == 0 {
            return EOK as _;
        }

        let (_bdev, bdif, dev) = unsafe { Self::dev_read_fields(bdev) };
        let buf_len = (bdif.ph_bsize * blk_cnt) as usize;
        let buffer = unsafe { slice::from_raw_parts_mut(buf as *mut u8, buf_len) };
        if let Err(err) = dev.read_blocks(blk_id, buffer) {
            error!("read_blocks failed: {err:?}");
            return EIO as _;
        }

        EOK as _
    }
    unsafe extern "C" fn dev_bwrite(
        bdev: *mut ext4_blockdev,
        buf: *const c_void,
        blk_id: u64,
        blk_cnt: u32,
    ) -> c_int {
        trace!("write ext4 block id={blk_id} count={blk_cnt}");
        if blk_cnt == 0 {
            return EOK as _;
        }

        let (_bdev, bdif, dev) = unsafe { Self::dev_read_fields(bdev) };
        let buf_len = (bdif.ph_bsize * blk_cnt) as usize;
        let buffer = unsafe { slice::from_raw_parts(buf as *const u8, buf_len) };
        if let Err(err) = dev.write_blocks(blk_id, buffer) {
            error!("read_blocks failed: {err:?}");
            return EIO as _;
        }

        // drop_cache();
        // sync

        EOK as _
    }
    unsafe extern "C" fn dev_close(_bdev: *mut ext4_blockdev) -> c_int {
        debug!("close ext4 block device");
        EOK as _
    }
}

impl<Dev: BlockDevice> Drop for Ext4BlockDevice<Dev> {
    fn drop(&mut self) {
        unsafe {
            let bdev = self.inner.as_mut();
            ext4_block_fini(bdev);
        }
    }
}
