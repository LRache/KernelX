use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;

use crate::driver::{DeviceType, DriverOps};
use crate::fs::devfs::devnode::CharDevInode;
use crate::fs::filesystem::{FileSystemOps, MountOptions, SuperBlockOps};
use crate::fs::memtreefs::inode::Inode as MemInode;
use crate::fs::{memtreefs, InodeOps, Mode, Owner};
use crate::kernel::errno::SysResult;
use crate::klib::InitedCell;

#[cfg(feature = "kvm")]
use super::inode::KvmInode;
use super::{LoopInode, NullInode, PtmxInode, RtcInode, URandomInode, ZeroInode};

pub struct DevfsInfo;
impl memtreefs::StaticFsInfo for DevfsInfo {
    fn type_name() -> &'static str {
        "devfs"
    }

    fn statfs_magic() -> u64 {
        0x9fa1
    }
}

static DEV_SUPERBLOCK: InitedCell<Arc<memtreefs::SuperBlock<DevfsInfo>>> = InitedCell::uninit();

pub struct FileSystem;

impl FileSystemOps for FileSystem {
    fn create(
        &self,
        _sno: u32,
        _driver: Option<Arc<dyn crate::driver::BlockDriverOps>>,
        _options: MountOptions,
    ) -> SysResult<Arc<dyn SuperBlockOps>> {
        Ok(DEV_SUPERBLOCK.clone())
    }
}

pub fn init() {
    let superblock = Arc::new(memtreefs::SuperBlock::new(false));
    let root = superblock.root_inode();
    root.add_child("null".into(), Arc::new(NullInode::new(superblock.alloc_inode_number())))
        .unwrap();
    root.add_child("zero".into(), Arc::new(ZeroInode::new(superblock.alloc_inode_number())))
        .unwrap();
    root.add_child(
        "urandom".into(),
        Arc::new(URandomInode::new(superblock.alloc_inode_number())),
    )
    .unwrap();
    #[cfg(feature = "kvm")]
    root.add_child("kvm".into(), Arc::new(KvmInode::new(superblock.alloc_inode_number())))
        .unwrap();
    let pts_dir = root
        .create(
            "pts",
            Mode::from_bits(Mode::S_IFDIR.bits() | 0o755).unwrap(),
            Owner::root(),
        )
        .unwrap();
    let pts_dir = pts_dir.downcast_arc::<MemInode<DevfsInfo>>().ok().unwrap();
    root.add_child(
        "ptmx".into(),
        Arc::new(PtmxInode::new(
            superblock.alloc_inode_number(),
            superblock.clone(),
            pts_dir.clone(),
        )),
    )
    .unwrap();

    // Create /dev/misc/ directory and add rtc
    let misc_dir = root
        .create(
            "misc",
            Mode::from_bits(Mode::S_IFDIR.bits() | 0o755).unwrap(),
            Owner::root(),
        )
        .unwrap();
    let misc_dir = misc_dir.downcast_arc::<MemInode<DevfsInfo>>().ok().unwrap();
    misc_dir
        .add_child("rtc".into(), Arc::new(RtcInode::new(superblock.alloc_inode_number())))
        .unwrap();

    // Create /dev/loop0 .. /dev/loop15
    for i in 0..16 {
        root.add_child(
            format!("loop{}", i),
            Arc::new(LoopInode::new(superblock.alloc_inode_number(), i)),
        )
        .unwrap();
    }

    DEV_SUPERBLOCK.init(superblock);
}

pub fn add_device(name: String, driver: Arc<dyn DriverOps>) {
    let root = DEV_SUPERBLOCK.root_inode();
    match driver.device_type() {
        DeviceType::Char => {
            let ino = DEV_SUPERBLOCK.alloc_inode_number();
            let cdev_inode = CharDevInode::new(ino, driver.as_char_driver().unwrap());
            root.add_child(name, Arc::new(cdev_inode)).unwrap();
        }
        DeviceType::Block => {
            let ino = DEV_SUPERBLOCK.alloc_inode_number();
            let bdev_inode = super::devnode::BlockDevInode::new(ino, driver.as_block_driver().unwrap());
            root.add_child(name, Arc::new(bdev_inode)).unwrap();
        }
        _ => {}
    }
}
