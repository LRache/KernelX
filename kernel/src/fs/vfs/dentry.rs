use alloc::sync::{Arc, Weak};
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;

use crate::fs::inode::{Index, LockedInode};
use crate::kernel::errno::SysResult;

use super::vfs;

struct DentryInner {
    pub name: String,
    pub parent: Option<Arc<Dentry>>,
    pub children: BTreeMap<String, Weak<Dentry>>,
    pub inode: Weak<LockedInode>,
    pub mount_to: Option<Arc<Dentry>>,
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
                mount_to: None,
            })
        }
    }

    pub fn new_noparent(name: &str, inode: &Arc<LockedInode>) -> Self {
        Self {
            inode_index: Index { sno: inode.get_sno(), ino: inode.get_ino() },
            inner: Mutex::new(DentryInner {
                name: name.into(), 
                parent: None,
                children: BTreeMap::new(),
                inode: Arc::downgrade(inode),
                mount_to: None,
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
                mount_to: None,
            })
        }
    }

    pub fn get_sno(&self) -> u32 {
        self.inode_index.sno
    }

    pub fn get_ino(&self) -> u32 {
        self.inode_index.ino
    }

    pub fn get_inode(&self) -> Arc<LockedInode> {
        let mut inner = self.inner.lock();
        match inner.inode.upgrade() {
            None => {
                let inode =  vfs().open_inode(&self.inode_index).expect("Failed to open inode from dentry");
                inner.inode = Arc::downgrade(&inode);
                inode
            }
            Some(inode) => inode,
        }
    }

    pub fn lookup(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>> {
        let mut inner = self.inner.lock();
        
        if let Some(mount_to) = &inner.mount_to {
            return mount_to.lookup(name);
        }
        
        if let Some(child) = inner.children.get(name) {
            if let Some(child) = child.upgrade() {
                return Ok(child);
            }
        }

        let inode = match inner.inode.upgrade() {
            None => {
                let inode =  vfs().open_inode(&self.inode_index)?;
                inner.inode = Arc::downgrade(&inode);
                inode
            }
            Some(inode) => inode,
        };

        let lookup_ino = inode.lookup(name)?;
        let lookup_sno = self.get_sno();
        let inode = vfs().open_inode(&Index { sno: lookup_sno, ino: lookup_ino })?;

        let new_child = Arc::new(Self::new(name, self, &inode));
        inner.children.insert(name.into(), Arc::downgrade(&new_child));
        
        Ok(new_child)
    }

    pub fn mount(self: &Arc<Self>, mount_to: &Arc<LockedInode>) {
        let mut inner = self.inner.lock();
        inner.mount_to = Some(Arc::new(
            Dentry { 
                inode_index: mount_to.get_index(), 
                inner: Mutex::new(DentryInner {
                    name: inner.name.clone(),
                    parent: inner.parent.clone(),
                    children: BTreeMap::new(),
                    inode: Arc::downgrade(mount_to),
                    mount_to: None,
                })
            }
        ));
    }
}
