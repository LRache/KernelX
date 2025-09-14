use alloc::boxed::Box;

use crate::fs::vfs;
use crate::fs::memfs::MemoryFileSystem;
use crate::fs::ext4::Ext4FileSystem;
use crate::{driver, println};

pub fn init() {
    vfs::init();

    vfs::register_filesystem("memfs", Box::new(MemoryFileSystem::new())).unwrap();
    vfs::register_filesystem("ext4", Box::new(Ext4FileSystem::new())).unwrap();

    let virtio_blk = driver::MANAGER.get_block_driver("virtio_block0").unwrap();
    
    vfs::mount("/", "ext4", Some(virtio_blk)).unwrap();

    println!("File system initialized and mounted successfully.");
}
