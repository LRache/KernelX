use alloc::string::String;
use alloc::sync::Arc;
use crate::driver::{DeviceType, DriverOps};
use crate::fs::devfs::devnode::CharDevInode;
use crate::fs::{filesystem::FileSystemOps, memtreefs};
use crate::klib::InitedCell;

use crate::fs::{InodeOps, Mode};
use crate::fs::memtreefs::inode::Inode as MemInode;
use super::{NullInode, ZeroInode, URandomInode, RtcInode};

struct DevfsInfo;
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
    fn create(&self, _sno: u32, _driver: Option<Arc<dyn crate::driver::BlockDriverOps>>) -> crate::kernel::errno::SysResult<Arc<dyn crate::fs::filesystem::SuperBlockOps>> {
        Ok(DEV_SUPERBLOCK.clone())
    }
}

pub fn init() {
    let superblock = memtreefs::SuperBlock::new();
    let root = superblock.root_inode();
    root.add_child("null".into(), Arc::new(NullInode::new(superblock.alloc_inode_number()))).unwrap();
    root.add_child("zero".into(), Arc::new(ZeroInode::new(superblock.alloc_inode_number()))).unwrap();
    root.add_child("urandom".into(), Arc::new(URandomInode::new(superblock.alloc_inode_number()))).unwrap();

    // Create /dev/misc/ directory and add rtc
    let misc_dir = root.create("misc", Mode::from_bits(Mode::S_IFDIR.bits() | 0o755).unwrap()).unwrap();
    let misc_dir = misc_dir.downcast_arc::<MemInode<DevfsInfo>>().ok().unwrap();
    misc_dir.add_child("rtc".into(), Arc::new(RtcInode::new(superblock.alloc_inode_number()))).unwrap();

    DEV_SUPERBLOCK.init(Arc::new(superblock));
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
