use crate::fs::filesystem::MountOptions;
use crate::fs::{Mode, Owner, devfs, vfs};
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
    vfs::mount(vfs::get_root_dentry(), path, fstype_name, None, MountOptions::default())
}

fn env_or(value: Option<&'static str>, default: &'static str) -> &'static str {
    match value {
        Some(value) if !value.is_empty() => value,
        _ => default,
    }
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
    const DEFAULT_SECOND_FSTYPE: &str = "ext4";
    const DEFAULT_SECOND_MOUNTPOINT: &str = "/mnt";

    let Some(device_name) = option_env!("KERNELX_SECOND_DEVICE").filter(|device_name| !device_name.is_empty()) else {
        return;
    };
    let fs_type = env_or(option_env!("KERNELX_SECOND_FSTYPE"), DEFAULT_SECOND_FSTYPE);
    let mountpoint = env_or(option_env!("KERNELX_SECOND_MOUNTPOINT"), DEFAULT_SECOND_MOUNTPOINT);
    let blk_dev = driver::get_block_driver(device_name).unwrap();

    ensure_mountpoint(mountpoint).unwrap();
    vfs::mount(
        vfs::get_root_dentry(),
        mountpoint,
        fs_type,
        Some(blk_dev),
        MountOptions::default(),
    )
    .unwrap();
    kinfo!("Second device {} mounted as {} on {}", device_name, fs_type, mountpoint);
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

    mount_second_device_if_enabled();

    kinfo!("Init filesystem mounted successfully!");
}

pub fn fini() {
    vfs::unmount_all().unwrap();
}
