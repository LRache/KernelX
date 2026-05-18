use alloc::boxed::Box;
use alloc::sync::Arc;
use core::ffi::{c_int, c_void};
use core::{mem, ptr, slice};

use super::ffi::*;
use crate::driver::BlockDriverOps;

pub const EXT4_DEV_BSIZE: usize = 512;

struct ResourceGuard {
    _driver: Box<Arc<dyn BlockDriverOps>>,
    _block_buf: Box<[u8; EXT4_DEV_BSIZE]>,
    _block_cache: Box<ext4_bcache>,
    _block_dev_iface: Box<ext4_blockdev_iface>,
}

pub struct Ext4BlockDevice {
    pub(crate) inner: Box<ext4_blockdev>,
    _guard: ResourceGuard,
}

impl Ext4BlockDevice {
    pub fn new(driver: Arc<dyn BlockDriverOps>) -> Result<Self, i32> {
        let mut driver = Box::new(driver);
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
            p_user: driver.as_mut() as *mut _ as *mut c_void,
        });
        let mut block_cache: Box<ext4_bcache> = Box::new(unsafe { mem::zeroed() });
        let mut blockdev = Box::new(ext4_blockdev {
            bdif: block_dev_iface.as_mut(),
            part_offset: 0,
            part_size: 0,
            bc: block_cache.as_mut(),
            lg_bsize: 0,
            lg_bcnt: 0,
            cache_write_back: 0,
            fs: ptr::null_mut(),
            journal: ptr::null_mut(),
        });

        unsafe {
            let rc = ext4_block_init(blockdev.as_mut());
            if rc != EOK as i32 {
                return Err(rc);
            }

            let rc = ext4_block_cache_write_back(blockdev.as_mut(), 1);
            if rc != EOK as i32 {
                ext4_block_fini(blockdev.as_mut());
                return Err(rc);
            }
        }

        Ok(Self {
            inner: blockdev,
            _guard: ResourceGuard {
                _driver: driver,
                _block_buf: block_buf,
                _block_cache: block_cache,
                _block_dev_iface: block_dev_iface,
            },
        })
    }

    unsafe fn dev_fields<'a>(
        bdev: *mut ext4_blockdev,
    ) -> (
        &'a mut ext4_blockdev,
        &'a mut ext4_blockdev_iface,
        &'a Arc<dyn BlockDriverOps>,
    ) {
        let bdev = unsafe { &mut *bdev };
        let bdif = unsafe { &mut *bdev.bdif };
        let driver = unsafe { &*(bdif.p_user as *const Arc<dyn BlockDriverOps>) };
        (bdev, bdif, driver)
    }

    unsafe extern "C" fn dev_open(bdev: *mut ext4_blockdev) -> c_int {
        let (bdev, bdif, driver) = unsafe { Self::dev_fields(bdev) };
        let ph_bcnt = match driver.get_block_size() {
            0 => return EIO as c_int,
            size => driver.get_block_count() as u64 * size as u64 / EXT4_DEV_BSIZE as u64,
        };

        bdif.ph_bcnt = ph_bcnt;
        bdev.part_offset = 0;
        bdev.part_size = ph_bcnt * bdif.ph_bsize as u64;
        EOK as c_int
    }

    unsafe extern "C" fn dev_bread(bdev: *mut ext4_blockdev, buf: *mut c_void, blk_id: u64, blk_cnt: u32) -> c_int {
        if blk_cnt == 0 {
            return EOK as c_int;
        }

        let (_bdev, bdif, driver) = unsafe { Self::dev_fields(bdev) };
        let buf_len = (bdif.ph_bsize * blk_cnt) as usize;
        let buffer = unsafe { slice::from_raw_parts_mut(buf.cast::<u8>(), buf_len) };
        if driver.read_at(blk_id as usize * EXT4_DEV_BSIZE, buffer).is_err() {
            return EIO as c_int;
        }

        EOK as c_int
    }

    unsafe extern "C" fn dev_bwrite(bdev: *mut ext4_blockdev, buf: *const c_void, blk_id: u64, blk_cnt: u32) -> c_int {
        if blk_cnt == 0 {
            return EOK as c_int;
        }

        let (_bdev, bdif, driver) = unsafe { Self::dev_fields(bdev) };
        let buf_len = (bdif.ph_bsize * blk_cnt) as usize;
        let buffer = unsafe { slice::from_raw_parts(buf.cast::<u8>(), buf_len) };
        if driver.write_at(blk_id as usize * EXT4_DEV_BSIZE, buffer).is_err() {
            return EIO as c_int;
        }

        EOK as c_int
    }

    unsafe extern "C" fn dev_close(_bdev: *mut ext4_blockdev) -> c_int {
        EOK as c_int
    }
}
