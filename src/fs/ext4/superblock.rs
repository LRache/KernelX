use core::ffi::c_void;
use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use ext4_rs::{BlockDevice, Ext4, Ext4DirSearchResult, BLOCK_SIZE};

use crate::kernel::errno::SysResult;
use crate::fs::ext4::inode::Ext4Inode;
use crate::fs::filesystem::SuperBlock;
use crate::fs::inode::Inode;
use crate::driver::block::BlockDriver;
use crate::{kinfo, print};

use super::ffi::*;

pub struct Ext4SuperBlock {
    // driver: Box<dyn BlockDriver>,
    sno: u32,
    // fs_handler: usize,
    superblock: Arc<Ext4>,
}

struct Disk {
    pub driver: Box<dyn BlockDriver>
}

impl BlockDevice for Disk {
    fn read_offset(&self, offset: usize) -> Vec<u8> {
        // let block_size = self.driver.get_block_size() as usize;
        // kinfo!("offset={}", offset);
        // assert!(offset % block_size as usize == 0);
        
        // let block = offset / block_size as usize;
        // let mut buf = vec![0u8; BLOCK_SIZE as usize];
        // for i in 0..(BLOCK_SIZE / block_size) {
        //     self.driver.read_block(
        //         block + i as usize, 
        //         &mut buf[(i as usize * block_size)..((i as usize + 1) * block_size)]
        //     ).expect("Failed to read block");
        // }
        let mut buf = vec![0u8; BLOCK_SIZE as usize];
        self.driver.read_at(offset, &mut buf).expect("Failed to read at offset");
        
        buf
    }

    fn write_offset(&self, offset: usize, buf: &[u8]) {
        // assert!(offset % BLOCK_SIZE as usize == 0);
        // let block = offset / BLOCK_SIZE as usize;
        // let block_size = self.driver.get_block_size() as usize;
        // for i in 0..(BLOCK_SIZE / block_size) {
        //     self.driver.write_block(
        //         block + i as usize, 
        //         &buf[(i as usize * block_size)..((i as usize + 1) * block_size)]
        //     ).expect("Failed to write block");
        // }
        self.driver.write_at(offset, buf).expect("Failed to write at offset");
    }
}

unsafe impl Send for Disk {}
unsafe impl Sync for Disk {}

// fn ffi_bopen(this: *mut c_void) -> i32 {
//     unsafe { &mut *(this as *mut Ext4SuperBlock) }.bopen()
// }

// fn ffi_bread(this: *mut c_void, buffer: *mut c_void, block_id: u64, block_count: u32) -> i32 {
//     unsafe { &mut *(this as *mut Ext4SuperBlock) }.bread(buffer, block_id, block_count)
// }

// fn ffi_bwrite(this: *mut c_void, buffer: *const c_void, block_id: u64, block_count: u32) -> i32 {
//     unsafe { &mut *(this as *mut Ext4SuperBlock) }.bwrite(buffer, block_id, block_count)
// }

// fn ffi_bclose(this: *mut c_void) -> i32 {
//     unsafe { &mut *(this as *mut Ext4SuperBlock) }.bclose()
// }

impl Ext4SuperBlock {
    pub fn new(sno: u32, driver: Box<dyn BlockDriver>) -> SysResult<Arc<Self>> {
        // let block_size = driver.get_block_size();
        // let block_count = driver.get_block_count();

        let superblock = Ext4::open(Arc::new(Disk{ driver }));

        Ok(Arc::new(Ext4SuperBlock {
            sno,
            // fs_handler: 0,
            superblock: Arc::new(superblock),
        }))
        
        // let mut this = Arc::new(Ext4SuperBlock {
        //     driver,
        //     sno,
        //     fs_handler: 0,
        // });

        // let mut this = Arc::new(Ext4SuperBlock {
        //     // driver,
        //     sno,
        //     // fs_handler: 0,
        //     superblock: Arc::new(Ext4::open(Arc::new(Disk{ driver }))),
        // });

        // Arc::get_mut(&mut this).unwrap().fs_handler = create_filesystem(
        //     block_size,
        //     block_count,
        //     ffi_bopen as usize,
        //     ffi_bread as usize,
        //     ffi_bwrite as usize,
        //     ffi_bclose as usize,
        //     & *this as *const Self as *mut Self,
        // )?;
        
        // Ok(this)
    }

    // fn bopen(&mut self) -> i32 {
    //     self.driver.open().map(|_| 0).unwrap_or(-1)
    // }

    // fn bread(&mut self, buffer: *mut c_void, block_id: u64, block_count: u32) -> i32 {
    //     let buffer = unsafe {
    //         core::slice::from_raw_parts_mut(
    //             buffer as *mut u8, 
    //             block_count as usize * self.driver.get_block_size() as usize
    //         )
    //     };
    //     self.driver.read_block(block_id as usize, buffer).map(|_| 0).expect("Failed to read block")
    // }

    // fn bwrite(&mut self, buffer: *const c_void, block_id: u64, block_count: u32) -> i32 {
    //     let buffer = unsafe {
    //         core::slice::from_raw_parts(
    //             buffer as *const u8, 
    //             block_count as usize * self.driver.get_block_size() as usize
    //         )
    //     };
    //     self.driver.write_block(block_id as usize, buffer).map(|_| 0).expect("Failed to write block")
    // }

    // fn bclose(&mut self) -> i32  {
    //     self.driver.close().map(|_| 0).expect("Failed to close block")
    // }

    pub fn get_superblock(&self) -> &Ext4 {
        &self.superblock
    }
}

unsafe impl Send for Ext4SuperBlock {}  
unsafe impl Sync for Ext4SuperBlock {}

impl SuperBlock for Ext4SuperBlock {
    fn get_inode(&self, ino: u32) -> SysResult<Box<dyn Inode>> {
        // Ok(Box::new(Ext4Inode::new(ino as u32, self.sno, self.fs_handler)?))
        Ok(Box::new(Ext4Inode::new(self.sno, ino, &self.superblock)))
    }

    fn get_root_ino(&self) -> u32 {
        2
    }

    fn unmount(&self) -> SysResult<()> {
        // destroy_filesystem(self.fs_handler)
        Ok(())
    }
}
