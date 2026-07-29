use crate::fs::filesystem::MountOptions;
use crate::fs::{Mode, Owner, devfs, vfs};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::{driver, kinfo};

#[unsafe(link_section = ".text.init")]
pub fn init() {
    kinfo!("Initializing file system...");

    vfs::init();
    devfs::init();

    kinfo!("File system initialized successfully.");
}

fn mount(path: &str, fstype_name: &str) -> SysResult<()> {
    let fstype = vfs::get_fstype(fstype_name).ok_or(Errno::ENODEV)?;
    vfs::mount(
        vfs::get_root_dentry(),
        vfs::get_root_dentry(),
        path,
        fstype,
        None,
        MountOptions::default(),
    )
}

fn ensure_mountpoint(path: &str) -> SysResult<()> {
    if vfs::load_dentry(path).is_ok() {
        return Ok(());
    }

    let Some(name) = path.strip_prefix('/') else {
        return Err(Errno::ENOENT);
    };
    if name.is_empty() || name.contains('/') {
        return Err(Errno::ENOENT);
    }

    match vfs::load_dentry("/")?.create(name, Mode::S_IFDIR | Mode::from_bits_truncate(0o755), Owner::root()) {
        Ok(_) | Err(Errno::EEXIST) => Ok(()),
        Err(err) => Err(err),
    }
}

#[unsafe(link_section = ".text.init")]
fn mount_second_device_if_enabled() {
    let Some(device_name) = config::DEFAULT_SECOND_DEVICE else {
        return;
    };

    if device_name.is_empty() {
        return;
    }

    let fs_type = config::DEFAULT_SECOND_FSTYPE;
    let fstype = vfs::get_fstype(fs_type).unwrap();
    let mountpoint = config::DEFAULT_SECOND_MOUNTPOINT;
    let blk_dev = driver::get_block_driver(device_name).unwrap();

    ensure_mountpoint(mountpoint).unwrap();
    vfs::mount(
        vfs::get_root_dentry(),
        vfs::get_root_dentry(),
        mountpoint,
        fstype,
        Some(blk_dev),
        MountOptions::default(),
    )
    .unwrap();
    kinfo!("Second device {} mounted as {} on {}", device_name, fs_type, mountpoint);
}

#[unsafe(link_section = ".text.init")]
pub fn mount_init_fs(device_name: &str, fs_type: &str) {
    let blk_dev = driver::get_block_driver(device_name).unwrap();
    let fstype = vfs::get_fstype(fs_type).unwrap();
    vfs::mount(
        vfs::get_root_dentry(),
        vfs::get_root_dentry(),
        "/",
        fstype,
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

    mount_second_device_if_enabled();

    kinfo!("Init filesystem mounted successfully!");
}

pub fn fini() {
    vfs::unmount_all().unwrap();
    vfs::fini();
}
