use core::fmt::Debug;

use alloc::sync::{Arc, Weak};
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;

use crate::kernel::errno::{SysResult, Errno};
use crate::fs::inode::{Index, InodeOps, Mode};

use super::vfs;

struct DentryInner {
    pub parent: Option<Arc<Dentry>>,
    pub children: BTreeMap<String, Weak<Dentry>>,
    pub inode: Weak<dyn InodeOps>,
    pub mount_to: Option<Arc<Dentry>>,
}

pub struct Dentry {
    inode_index: Index,
    name: String,
    inner: Mutex<DentryInner>,
}

impl Dentry {
    pub fn new(name: &str, parent: &Arc<Dentry>, inode: &Arc<dyn InodeOps>) -> Self {
        Self {
            inode_index: Index { sno: inode.get_sno(), ino: inode.get_ino() },
            name: name.into(),
            inner: Mutex::new(DentryInner {
                parent: Some(parent.clone()),
                children: BTreeMap::new(),
                inode: Arc::downgrade(inode),
                mount_to: None,
            })
        }
    }

    pub fn root(inode: &Arc<dyn InodeOps>) -> Self {
        Self {
            inode_index: Index { sno: inode.get_sno(), ino: inode.get_ino() },
            name: "/".into(),
            inner: Mutex::new(DentryInner {
                parent: None,
                children: BTreeMap::new(),
                inode: Arc::downgrade(inode),
                mount_to: None,
            })
        }
    }

    pub fn sno(&self) -> u32 {
        self.inode_index.sno
    }

    pub fn ino(&self) -> u32 {
        self.inode_index.ino
    }

    fn get_inode_inner(&self, inner: &mut DentryInner) -> SysResult<Arc<dyn InodeOps>> {
        match inner.inode.upgrade() {
            None => {
                let inode =  vfs().load_inode(self.sno(), self.ino())?;
                inner.inode = Arc::downgrade(&inode);
                Ok(inode)
            }
            Some(inode) => Ok(inode),
        }
    }

    pub fn get_inode(&self) -> Arc<dyn InodeOps> {
        self.get_inode_inner(&mut self.inner.lock()).expect("Failed to get inode from dentry")
    }

    pub fn get_parent(&self) -> Option<Arc<Dentry>> {
        let inner = self.inner.lock();
        inner.parent.clone()
    }

    pub fn lookup(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>> {
        let mut inner = self.inner.lock();
        
        if let Some(child) = inner.children.get(name) {
            if let Some(child) = child.upgrade() {
                return Ok(child);
            }
        }

        let inode = match inner.inode.upgrade() {
            None => {
                let inode = vfs().load_inode(self.sno(), self.ino())?;
                inner.inode = Arc::downgrade(&inode);
                inode
            }
            Some(inode) => inode,
        };

        let lookup_ino = inode.lookup(name)?;
        let lookup_sno = self.sno();
        let inode = vfs().load_inode(lookup_sno, lookup_ino)?;

        let new_child = Arc::new(Self::new(name, self, &inode));
        inner.children.insert(name.into(), Arc::downgrade(&new_child));
        
        Ok(new_child)
    }

    pub fn get_mount_to(self: Arc<Self>) -> Arc<Dentry> {
        if let Some(mount_to) = &self.inner.lock().mount_to {
            mount_to.clone()
        } else {
            self
        }
    }

    pub fn get_link_to(self: Arc<Self>) -> Arc<Dentry> {
        // TODO: Symbolic link
        self
    }

    pub fn mount(self: &Arc<Self>, mount_to: &Arc<dyn InodeOps>) {
        let mut inner = self.inner.lock();
        inner.mount_to = Some(Arc::new(
            Dentry { 
                inode_index: Index { sno: mount_to.get_sno(), ino: mount_to.get_ino() },
                name: self.name.clone(),
                inner: Mutex::new(DentryInner {
                    parent: inner.parent.clone(),
                    children: BTreeMap::new(),
                    inode: Arc::downgrade(mount_to),
                    mount_to: None,
                })
            }
        ));
    }

    pub fn get_path(&self) -> String {
        let inner = self.inner.lock();
        if let Some(parent) = &inner.parent {
            let mut path = parent.get_path();
            if !path.ends_with('/') {
                path.push('/');
            }
            if self.name != "/" {
                path.push_str(&self.name);
            }
            path
        } else {
            self.name.clone()
        }
    }

    pub fn create(self: &Arc<Self>, name: &str, mode: Mode) -> SysResult<()> {
        if let Ok(_) = self.lookup(name) {
            return Err(Errno::EEXIST);
        }

        let mut inner = self.inner.lock();
        
        if let Some(mount_to) = &inner.mount_to {
            return mount_to.create(name, mode);
        }

        let inode = self.get_inode_inner(&mut inner)?;

        inode.create(name, mode)
    }

    pub fn unlink(self: &Arc<Self>, name: &str) -> SysResult<()> {
        let mut inner = self.inner.lock();
        
        if let Some(mount_to) = &inner.mount_to {
            return mount_to.unlink(name);
        }

        let inode = match inner.inode.upgrade() {
            None => {
                let inode =  vfs().load_inode(self.sno(), self.ino())?;
                inner.inode = Arc::downgrade(&inode);
                inode
            }
            Some(inode) => inode,
        };

        inode.unlink(name)?;

        inner.children.remove(name);

        Ok(())
    }

    pub fn rename(self: &Arc<Self>, old_name: &str, new_parent: &Arc<Dentry>, new_name: &str) -> SysResult<()> {
        assert!(self.sno() == new_parent.sno());
        assert!(old_name != "." && old_name != "..");
        assert!(new_name != "." && new_name != "..");

        let old_parent_inode = self.get_inode();
        let new_parent_inode = new_parent.get_inode();
        old_parent_inode.rename(old_name, &new_parent_inode, new_name)?;

        self.inner.lock().children.remove(old_name);

        Ok(())
    }

    pub fn readlink(&self) -> SysResult<String> {
        let mut inner = self.inner.lock();
        
        if let Some(mount_to) = &inner.mount_to {
            return mount_to.readlink();
        }

        let inode = self.get_inode_inner(&mut inner)?;

        inode.readlink()
    }
}

impl Debug for Dentry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Dentry {{ sno: {}, ino: {}, name: {} }}", self.sno(), self.ino(), self.name)
    }
}

impl Drop for Dentry {
    fn drop(&mut self) {
    }
}
