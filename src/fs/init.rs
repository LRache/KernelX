use alloc::boxed::Box;

use crate::fs::vfs;
// use crate::fs::memfs::MemoryFileSystem;
use crate::fs::ext4::Ext4FileSystem;
use crate::fs::devfs::DevFileSystem;
use crate::driver;
use crate::fs::Mode;
use crate::kinfo;

pub fn init() {
    kinfo!("Initializing file system...");

    vfs::init();

    vfs::register_filesystem("devfs", Box::new(DevFileSystem::new())).unwrap();
    // vfs::register_filesystem("memfs", Box::new(MemoryFileSystem::new())).unwrap();
    vfs::register_filesystem("ext4", Box::new(Ext4FileSystem::new())).unwrap();

    kinfo!("File systems registered successfully.");

    let virtio_blk = driver::MANAGER.get_block_driver("virtio_block0").unwrap();
    
    vfs::mount("/", "ext4", Some(virtio_blk)).unwrap();
    
    // Mount devfs at /dev
    let _ = vfs::load_dentry("/").unwrap().create("dev", Mode::S_IFDIR);
    vfs::mount("/dev", "devfs", None).unwrap();

    // Try to access /dev/null and /dev/zero to ensure they are working
    vfs::load_dentry("/dev/null").unwrap();
    vfs::load_dentry("/dev/zero").unwrap();

    kinfo!("File system initialized and mounted successfully.");
}

pub fn fini() {
    vfs::fini();
    // vfs::unmount_all().unwrap();
}
