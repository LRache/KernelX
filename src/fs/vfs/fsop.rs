use alloc::boxed::Box;

use crate::driver::block::BlockDevice;
use crate::fs::filesystem::FileSystem;
use crate::fs::vfs::VFS;
use crate::kernel::errno::Errno;

pub fn register_filesystem(typename: &str, fs: Box<dyn FileSystem>) -> Result<(), ()> {
    let mut fstype = VFS.fstype_map.lock();
    if fstype.contains_key(typename) {
        return Err(());
    }
    fstype.insert(typename.into(), fs);
    Ok(())
}

pub fn mount(path: &str, fstype_name: &str, device: Option<Box<dyn BlockDevice>>) -> Result<(), Errno> {
    let index = VFS.lookup_inode(path, 0, 0)?;

    let fstype_map = VFS.fstype_map.lock();
    let fstype = fstype_map.get(fstype_name).ok_or(Errno::ENOENT)?;

    let mut fs_table = VFS.superblock_table.lock();
    let fsno = fs_table.next_fsno();
    
    let superblock = fstype.create(fsno, device)?;
    let root_ino = superblock.get_root_inode().get_ino();

    fs_table.push(superblock);

    let mut mountmanager = VFS.mountmanager.lock();
    mountmanager.add_mount(index.fsno, index.ino, fsno, root_ino);

    Ok(())
}