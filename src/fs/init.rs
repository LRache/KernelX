use alloc::boxed::Box;

use crate::fs::vfs;
use crate::fs::memfs::MemoryFileSystem;
use crate::fs::ext4::Ext4FileSystem;
use crate::{driver, kinfo, println};

pub fn init() {
    kinfo!("Initializing file system...");

    vfs::init();

    vfs::register_filesystem("memfs", Box::new(MemoryFileSystem::new())).unwrap();
    vfs::register_filesystem("ext4", Box::new(Ext4FileSystem::new())).unwrap();

    kinfo!("File systems registered successfully.");

    let virtio_blk = driver::MANAGER.get_block_driver("virtio_block0").unwrap();
    
    vfs::mount("/", "ext4", Some(virtio_blk)).unwrap();

    println!("File system initialized and mounted successfully.");
}

pub fn fini() {
    vfs::fini();
    vfs::unmount_all().unwrap();
}
