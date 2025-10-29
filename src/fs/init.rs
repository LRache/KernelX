use crate::fs::vfs;
use crate::driver;
use crate::fs::Mode;
use crate::kinfo;

#[unsafe(link_section = ".text.init")]
pub fn init() {
    kinfo!("Initializing file system...");

    vfs::init();

    kinfo!("File system initialized successfully.");
}

#[unsafe(link_section = ".text.init")]
pub fn mount_init_fs(device_name: &str, fs_type: &str) {
    let blk_dev = driver::get_block_driver(device_name).unwrap();
    vfs::mount("/", fs_type, Some(blk_dev)).unwrap();

    // Mount devfs at /dev
    let _ = vfs::load_dentry("/").unwrap().create("dev", Mode::S_IFDIR);
    vfs::mount("/dev", "devfs", None).unwrap();
    
    // Mount tmpfs at /tmp
    let _ = vfs::load_dentry("/").unwrap().create("tmp", Mode::S_IFDIR);
    vfs::mount("/tmp", "tmpfs", None).unwrap();

    // Try to access /dev/null and /dev/zero to ensure they are working
    vfs::load_dentry("/dev/null").unwrap();
    vfs::load_dentry("/dev/zero").unwrap();

    kinfo!("Init filesystem mounted successfully!");
}

pub fn fini() {
    // vfs::fini();
    // vfs::unmount_all().unwrap();
}
