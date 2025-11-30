use core::fmt::Debug;

use alloc::sync::{Arc, Weak};
use alloc::string::String;
use alloc::collections::BTreeMap;

use crate::kernel::errno::{SysResult, Errno};
use crate::fs::inode::{Index, InodeOps, Mode};
use crate::klib::SpinLock;

use super::vfs;

pub struct Dentry {
    inode_index: Index,
    name: String,
    parent: SpinLock<Option<Arc<Dentry>>>,
    children: SpinLock<BTreeMap<String, Weak<Dentry>>>,
    inode: SpinLock<Weak<dyn InodeOps>>,
    mount_to: SpinLock<Option<Arc<Dentry>>>,
}

impl Dentry {
    pub fn new(name: &str, parent: &Arc<Dentry>, inode: &Arc<dyn InodeOps>) -> Self {
        Self {
            inode_index: Index { sno: inode.get_sno(), ino: inode.get_ino() },
            name: name.into(),
            parent: SpinLock::new(Some(parent.clone())),
            children: SpinLock::new(BTreeMap::new()),
            inode: SpinLock::new(Arc::downgrade(inode)),
            mount_to: SpinLock::new(None),
        }
    }

    pub fn root(inode: &Arc<dyn InodeOps>) -> Self {
        Self {
            inode_index: Index { sno: inode.get_sno(), ino: inode.get_ino() },
            name: "/".into(),
            parent: SpinLock::new(None),
            children: SpinLock::new(BTreeMap::new()),
            inode: SpinLock::new(Arc::downgrade(inode)),
            mount_to: SpinLock::new(None),
        }
    }

    pub fn sno(&self) -> u32 {
        self.inode_index.sno
    }

    pub fn ino(&self) -> u32 {
        self.inode_index.ino
    }

    pub fn get_inode(&self) -> Arc<dyn InodeOps> {
        let inode = self.inode.lock();
        match inode.upgrade() {
            None => {
                drop(inode);
                let inode =  vfs().load_inode(self.sno(), self.ino()).expect("Failed to load inode");
                *self.inode.lock() = Arc::downgrade(&inode);
                inode
            }
            Some(inode) => inode,
        }
    }

    pub fn get_parent(&self) -> Option<Arc<Dentry>> {
        (*self.parent.lock()).clone()
    }

    pub fn lookup(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>> {
        let mut children = self.children.lock();

        if let Some(child) = children.get(name) {
            if let Some(child) = child.upgrade() {
                return Ok(child);
            }
        }
        let lookup_ino = self.get_inode().lookup(name)?;
        let lookup_sno = self.sno();
        let inode = vfs().load_inode(lookup_sno, lookup_ino)?;

        let new_child = Arc::new(Self::new(name, self, &inode));
        children.insert(name.into(), Arc::downgrade(&new_child));
        
        Ok(new_child)
    }

    pub fn get_mount_to(self: Arc<Self>) -> Arc<Dentry> {
        if let Some(mount_to) = &*self.mount_to.lock() {
            mount_to.clone()
        } else {
            self
        }
    }

    pub fn walk_link(self: Arc<Self>) -> SysResult<Arc<Dentry>> {
        let parent = self.parent.lock();
        if let Some(p) = &*parent {
            let inode = self.get_inode();
            if inode.mode()?.contains(Mode::S_IFLNK) {
                let mut buffer = [0u8; 255];
                let count = inode.readat(&mut buffer, 0)?;
                let link_path = core::str::from_utf8(&buffer[..count]).map_err(|_| Errno::EINVAL)?;
                let r = vfs().lookup_dentry(&p, &link_path)?;
                Ok(r)
            } else {
                drop(parent);
                Ok(self)
            }
        } else {
            drop(parent);
            Ok(self)
        }
    }

    pub fn mount(self: &Arc<Self>, mount_to: &Arc<dyn InodeOps>) {
        *self.mount_to.lock() =  Some(Arc::new(
            Dentry { 
                inode_index: Index { sno: mount_to.get_sno(), ino: mount_to.get_ino() },
                name: self.name.clone(),
                parent: SpinLock::new(self.parent.lock().clone()),
                children: SpinLock::new(BTreeMap::new()),
                inode: SpinLock::new(Arc::downgrade(mount_to)),
                mount_to: SpinLock::new(None),
            }
        ));
    }

    pub fn get_path(&self) -> String {
        let parent = self.parent.lock();
        if let Some(parent) = &*parent {
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

        let inode = self.get_inode();

        inode.create(name, mode)
    }

    pub fn unlink(self: &Arc<Self>, name: &str) -> SysResult<()> {
        self.get_inode().unlink(name)?;

        self.children.lock().remove(name);
        
        Ok(())
    }

    pub fn rename(self: &Arc<Self>, old_name: &str, new_parent: &Arc<Dentry>, new_name: &str) -> SysResult<()> {
        assert!(self.sno() == new_parent.sno());
        assert!(old_name != "." && old_name != "..");
        assert!(new_name != "." && new_name != "..");

        let old_parent_inode = self.get_inode();
        let new_parent_inode = new_parent.get_inode();
        old_parent_inode.rename(old_name, &new_parent_inode, new_name)?;

        self.children.lock().remove(old_name);

        Ok(())
    }

    pub fn readlink(&self) -> SysResult<String> {
        // let mut inner = self.inner.lock();
        
        // if let Some(mount_to) = &inner.mount_to {
        //     return mount_to.readlink();
        // }

        // let inode = self.get_inode_inner(&mut inner)?;

        // inode.readlink()
        Ok("".into()) // TODO
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
