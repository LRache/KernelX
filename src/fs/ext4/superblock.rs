use core::ffi::c_void;
use alloc::sync::Arc;
use alloc::boxed::Box;

use crate::driver::block::{BlockDevice, BlockDriver};
use crate::fs::ext4::inode::Ext4Inode;
use crate::fs::filesystem::SuperBlock;
use crate::fs::inode::Inode;
use crate::kernel::errno::Errno;
use super::ffi::*;

pub struct Ext4SuperBlock {
    driver: Box<dyn BlockDriver>,
    fsno: usize,
    fs_handler: usize,
}

fn ffi_bopen(this: *mut c_void) -> i32 {
    unsafe { &mut *(this as *mut Ext4SuperBlock) }.bopen()
}

fn ffi_bread(this: *mut c_void, buffer: *mut c_void, block_id: u64, block_count: u32) -> i32 {
    unsafe { &mut *(this as *mut Ext4SuperBlock) }.bread(buffer, block_id, block_count)
}

fn ffi_bwrite(this: *mut c_void, buffer: *const c_void, block_id: u64, block_count: u32) -> i32 {
    unsafe { &mut *(this as *mut Ext4SuperBlock) }.bwrite(buffer, block_id, block_count)
}

fn ffi_bclose(this: *mut c_void) -> i32 {
    unsafe { &mut *(this as *mut Ext4SuperBlock) }.bclose()
}

impl Ext4SuperBlock {
    pub fn new(fsno: usize, device: Box<dyn BlockDevice>) -> Result<Arc<Self>, Errno> {
        let driver = device.driver();
        let block_size = driver.get_block_size();
        let block_count = driver.get_block_count();
        
        let mut this = Arc::new(Ext4SuperBlock {
            driver,
            fsno,
            fs_handler: 0,
        });

        Arc::get_mut(&mut this).unwrap().fs_handler = create_filesystem(
            block_size,
            block_count,
            ffi_bopen as usize,
            ffi_bread as usize,
            ffi_bwrite as usize,
            ffi_bclose as usize,
            & *this as *const Self as *mut Self,
        )?;
        
        Ok(this)
    }

    fn bopen(&mut self) -> i32 {
        self.driver.open().map(|_| 0).unwrap_or(-1)
    }

    fn bread(&mut self, buffer: *mut c_void, block_id: u64, block_count: u32) -> i32 {
        let buffer = unsafe {
            core::slice::from_raw_parts_mut(
                buffer as *mut u8, 
                block_count as usize * self.driver.get_block_size() as usize
            )
        };
        self.driver.read_block(block_id as usize, buffer).map(|_| 0).expect("Failed to read block")
    }

    fn bwrite(&mut self, buffer: *const c_void, block_id: u64, block_count: u32) -> i32 {
        let buffer = unsafe {
            core::slice::from_raw_parts(
                buffer as *const u8, 
                block_count as usize * self.driver.get_block_size() as usize
            )
        };
        self.driver.write_block(block_id as usize, buffer).map(|_| 0).expect("Failed to write block")
    }

    fn bclose(&mut self) -> i32  {
        self.driver.close().map(|_| 0).expect("Failed to close block")
    }
}

unsafe impl Send for Ext4SuperBlock {}  
unsafe impl Sync for Ext4SuperBlock {}

impl SuperBlock for Ext4SuperBlock {  
    fn get_inode(&self, ino: usize) -> Result<Box<dyn Inode>, Errno> {
        Ok(Box::new(Ext4Inode::new(ino as u32, self.fsno, self.fs_handler)?))
    }

    fn get_root_inode(&self) -> Box<dyn Inode> {
        let root_ino = 2;
        self.get_inode(root_ino).expect("Failed to get root inode")
    }
}

impl Drop for Ext4SuperBlock {
    fn drop(&mut self) {
        let _ = destroy_filesystem(self.fs_handler);
    }
}
