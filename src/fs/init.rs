use crate::fs::filesystem::MountOptions;
use crate::fs::{Mode, Owner, devfs, vfs};
use crate::kernel::errno::SysResult;
use crate::{driver, kinfo};

#[unsafe(link_section = ".text.init")]
pub fn init() {
    kinfo!("Initializing file system...");

    vfs::init();
    devfs::init();

    kinfo!("File system initialized successfully.");
}

fn mount(path: &str, fstype_name: &str) -> SysResult<()> {
    vfs::mount(vfs::get_root_dentry(), path, fstype_name, None, MountOptions::default())
}

#[unsafe(link_section = ".text.init")]
pub fn mount_init_fs(device_name: &str, fs_type: &str) {
    let blk_dev = driver::get_block_driver(device_name).unwrap();
    vfs::mount(
        vfs::get_root_dentry(),
        "/",
        fs_type,
        Some(blk_dev),
        MountOptions::default(),
    )
    .unwrap();

    // Mount devfs at /dev
    let _ =
        vfs::load_dentry("/")
            .unwrap()
            .create("dev", Mode::S_IFDIR | Mode::from_bits_truncate(0o755), Owner::root());
    let _ =
        vfs::load_dentry("/")
            .unwrap()
            .create("proc", Mode::S_IFDIR | Mode::from_bits_truncate(0o755), Owner::root());
    mount("/dev", "devfs").unwrap();
    mount("/proc", "procfs").unwrap();

    // Try to access /dev/null and /dev/zero to ensure they are working
    vfs::load_dentry("/dev/null").unwrap();
    vfs::load_dentry("/dev/zero").unwrap();

    // Mount tmpfs at /tmp
    let _ =
        vfs::load_dentry("/")
            .unwrap()
            .create("tmp", Mode::S_IFDIR | Mode::from_bits_truncate(0o755), Owner::root());
    vfs::mount(vfs::get_root_dentry(), "/tmp", "tmpfs", None, MountOptions::default()).unwrap();

    let _ =
        vfs::load_dentry("/")
            .unwrap()
            .create("var", Mode::S_IFDIR | Mode::from_bits_truncate(0o755), Owner::root());
    let _ =
        vfs::load_dentry("/var")
            .unwrap()
            .create("tmp", Mode::S_IFDIR | Mode::from_bits_truncate(0o755), Owner::root());
    mount("/var/tmp", "tmpfs").unwrap();

    kinfo!("Init filesystem mounted successfully!");
}

pub fn fini() {
    vfs::unmount_all().unwrap();
}
