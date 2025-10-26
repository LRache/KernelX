use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::kernel::errno::Errno;
use crate::fs::filesystem::FileSystem;
use crate::driver::BlockDriverOps;

use super::vfs;
use super::Dentry;

pub fn register_filesystem(typename: &str, fs: Box<dyn FileSystem>) -> Result<(), ()> {
    let mut fstype = vfs().fstype_map.lock();
    if fstype.contains_key(typename) {
        return Err(());
    }
    fstype.insert(typename.into(), fs);
    Ok(())
}

pub fn mount(path: &str, fstype_name: &str, device: Option<Arc<dyn BlockDriverOps>>) -> Result<(), Errno> {
    vfs().mount(path, fstype_name, device)
}

pub fn get_root_dentry() -> &'static Arc<Dentry> {
    vfs().get_root()
}

// pub fn unmount_all() -> Result<(), Errno> {
//     let superblock_table = vfs().superblock_table.lock();
//     superblock_table.unmount_all()?;

//     Ok(())
// }
