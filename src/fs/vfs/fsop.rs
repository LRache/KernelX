use alloc::sync::Arc;

use crate::fs::vfs::vfs::VirtualFileSystem;
use crate::fs::filesystem::FileSystemOps;
use crate::kdebug;
use crate::kernel::errno::{Errno, SysResult};
use crate::driver::BlockDriverOps;

use super::vfs;
use super::Dentry;

impl VirtualFileSystem {
    pub(super) fn register_filesystem(&mut self, name: &'static str, fs: &'static dyn FileSystemOps) {
        self.fstype_map.insert(name, fs);
    }

    fn mount(&self, path: &str, fstype_name: &str, device: Option<Arc<dyn BlockDriverOps>>) -> SysResult<()> {
        let dentry = self.lookup_dentry(self.get_root(), path)?;

        let fstype = self.fstype_map.get(fstype_name).ok_or(Errno::ENOENT)?;

        let (sno, root_ino) = {
            let mut superblock_table = self.superblock_table.lock();
            let sno = superblock_table.alloc(*fstype, device)?;
            (sno, superblock_table.get(sno).unwrap().get_root_ino())
        };

        let root_inode = self.load_inode(sno, root_ino)?;

        dentry.mount(&root_inode);
        
        self.mountpoint.lock().push(dentry);
        
        Ok(())
    }
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
