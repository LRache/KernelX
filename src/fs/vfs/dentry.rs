use alloc::sync::{Arc, Weak};
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;

use crate::fs::inode::inode::LockedInode;
use crate::fs::vfs::vfs::VFS;
use crate::kernel::errno::SysResult;

pub struct Dentry {
    name: String,
    parent: Option<Arc<Dentry>>,
    children: Mutex<BTreeMap<String, Weak<Dentry>>>,
    inode: Arc<LockedInode>
}

impl Dentry {
    pub fn new(name: &str, parent: &Arc<Dentry>, inode: &Arc<LockedInode>) -> Self {
        Self {
            name: name.into(),
            parent: Some(parent.clone()),
            children: Mutex::new(BTreeMap::new()),
            inode: inode.clone()
        }
    }

    pub fn get_inode(&self) -> &Arc<LockedInode> {
        &self.inode
    }

    pub fn lookup(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>> {
        let mut children = self.children.lock();
        if let Some(child) = children.get(name) {
            if let Some(child) = child.upgrade() {
                return Ok(child.clone());
            }
        }

        let ino = self.inode.lookup(name)?;
        let sno = self.inode.get_sno();
        let inode = VFS.open_inode(sno, ino)?;

        let new_child = Arc::new(Self::new(name, self, &inode));
        children.insert(name.into(), Arc::downgrade(&new_child));
        Ok(new_child)
    }
}
