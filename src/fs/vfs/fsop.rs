use alloc::boxed::Box;

use crate::kernel::errno::Errno;
use crate::driver::block::BlockDriver;
use crate::fs::filesystem::FileSystem;
use crate::fs::inode;

use super::vfs;

pub fn register_filesystem(typename: &str, fs: Box<dyn FileSystem>) -> Result<(), ()> {
    let mut fstype = vfs().fstype_map.lock();
    if fstype.contains_key(typename) {
        return Err(());
    }
    fstype.insert(typename.into(), fs);
    Ok(())
}

pub fn mount(path: &str, fstype_name: &str, device: Option<Box<dyn BlockDriver>>) -> Result<(), Errno> {
    let dentry = vfs().lookup_dentry(vfs().get_root(), path)?;

    let fstype_map = vfs().fstype_map.lock();
    let fstype = fstype_map.get(fstype_name).ok_or(Errno::ENOENT)?;

    let (sno, root_ino) = {
        let mut superblock_table = vfs().superblock_table.lock();
        let sno = superblock_table.alloc(fstype, device)?;
        (sno, superblock_table.get(sno).unwrap().get_root_ino())
    };

    let root_inode = vfs().open_inode(&inode::Index { sno, ino: root_ino })?;

    dentry.mount(&root_inode);

    Ok(())
}

pub fn unmount_all() -> Result<(), Errno> {
    let superblock_table = vfs().superblock_table.lock();
    superblock_table.unmount_all()?;

    Ok(())
}
