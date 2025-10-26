use alloc::boxed::Box;

use crate::fs::vfs;
// use crate::fs::memfs::MemoryFileSystem;
use crate::fs::ext4::Ext4FileSystem;
use crate::fs::devfs::DevFileSystem;
use crate::driver;
use crate::fs::Mode;
use crate::kinfo;

// const INIT_BLOCK_DEVICE_NAME: &str = "sdio1";
const INIT_BLOCK_DEVICE_NAME: &str = "virtio_block0";
const INIT_FILESYSTEM_TYPE: &str = "ext4";

#[unsafe(link_section = ".text.init")]
pub fn init() {
    kinfo!("Initializing file system...");

    vfs::init();

    vfs::register_filesystem("devfs", Box::new(DevFileSystem::new())).unwrap();
    // vfs::register_filesystem("memfs", Box::new(MemoryFileSystem::new())).unwrap();
    vfs::register_filesystem("ext4", Box::new(Ext4FileSystem::new())).unwrap();

    kinfo!("File systems registered successfully.");

    let init_blk = driver::get_block_driver(INIT_BLOCK_DEVICE_NAME).unwrap();

    vfs::mount("/", INIT_FILESYSTEM_TYPE, Some(init_blk)).unwrap();

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
