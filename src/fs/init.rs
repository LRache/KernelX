use alloc::boxed::Box;

use crate::driver::block::{BlockDevice, VirtIOBlockDevice};
use crate::fs::vfs;
use crate::fs::memfs::MemoryFileSystem;
use crate::fs::ext4::Ext4FileSystem;
use crate::println;

pub fn init() {
    vfs::init();

    vfs::register_filesystem("memfs", Box::new(MemoryFileSystem::new())).unwrap();
    vfs::register_filesystem("ext4", Box::new(Ext4FileSystem::new())).unwrap();

    let virtio_blk = Box::new(VirtIOBlockDevice::new(0x1000_1000)) as Box<dyn BlockDevice>;

    // vfs::mount("/", "memfs", None).unwrap();
    vfs::mount("/", "ext4", Some(virtio_blk)).unwrap();

    println!("File system initialized and mounted successfully.");
}
