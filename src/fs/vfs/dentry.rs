use alloc::sync::{Arc, Weak};
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;

use crate::fs::inode::{Index, LockedInode};
use crate::kernel::errno::SysResult;

use super::VFS;

struct DentryInner {
    pub name: String,
    pub parent: Option<Arc<Dentry>>,
    pub children: BTreeMap<String, Weak<Dentry>>,
    pub inode: Weak<LockedInode>,
}

pub struct Dentry {
    inode_index: Index,
    inner: Mutex<DentryInner>,
}

impl Dentry {
    pub fn new(name: &str, parent: &Arc<Dentry>, inode: &Arc<LockedInode>) -> Self {
        Self {
            inode_index: Index { sno: inode.get_sno(), ino: inode.get_ino() },
            inner: Mutex::new(DentryInner {
                name: name.into(),
                parent: Some(parent.clone()),
                children: BTreeMap::new(),
                inode: Arc::downgrade(inode),
            })
        }
    }

    pub fn root(inode: &Arc<LockedInode>) -> Self {
        Self {
            inode_index: Index { sno: inode.get_sno(), ino: inode.get_ino() },
            inner: Mutex::new(DentryInner {
                name: "/".into(),
                parent: None,
                children: BTreeMap::new(),
                inode: Arc::downgrade(inode),
            })
        }
    }

    pub fn get_sno(&self) -> u32 {
        self.inode_index.sno
    }

    pub fn get_ino(&self) -> u32 {
        self.inode_index.ino
    }

    pub fn lookup(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>> {
        let mut inner = self.inner.lock();
        if let Some(child) = inner.children.get(name) {
            if let Some(child) = child.upgrade() {
                return Ok(child.clone());
            }
        }

        let inode = match inner.inode.upgrade() {
            None => {
                let inode =  VFS.open_inode(self.get_sno(), self.get_ino())?;
                inner.inode = Arc::downgrade(&inode);
                inode
            }
            Some(inode) => inode,
        };

        let lookup_ino = inode.lookup(name)?;
        let lookup_sno = self.get_sno();
        let inode = VFS.open_inode(lookup_sno, lookup_ino)?;

        let new_child = Arc::new(Self::new(name, self, &inode));
        inner.children.insert(name.into(), Arc::downgrade(&new_child));
        
        Ok(new_child)
    }
}
