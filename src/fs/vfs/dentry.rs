use core::fmt::Debug;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};

use crate::fs::inode::{Index, InodeOps, Mode, Owner};
use crate::kernel::errno::{Errno, SysResult};
use crate::klib::SpinLock;

use super::vfs;

pub struct Dentry {
    inode_index: Index,
    name: String,
    parent: Option<Arc<Dentry>>,
    children: SpinLock<BTreeMap<String, Weak<Dentry>>>,
    inode: SpinLock<Weak<dyn InodeOps>>,
    mount_to: SpinLock<Option<Arc<Dentry>>>,
}

impl Dentry {
    pub fn new(name: &str, parent: &Arc<Dentry>, inode: &Arc<dyn InodeOps>, sno: u32) -> Self {
        Self {
            inode_index: Index {
                sno: sno,
                ino: inode.get_ino(),
            },
            name: name.into(),
            parent: Some(parent.clone()),
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(inode), "Dentry::inode"),
            mount_to: SpinLock::new(None, "Dentry::mount_to"),
        }
    }

    pub fn root(inode: &Arc<dyn InodeOps>, sno: u32) -> Self {
        Self {
            inode_index: Index {
                sno,
                ino: inode.get_ino(),
            },
            name: "/".into(),
            parent: None,
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(inode), "Dentry::inode"),
            mount_to: SpinLock::new(None, "Dentry::mount_to"),
        }
    }

    pub fn sno(&self) -> u32 {
        self.inode_index.sno
    }

    pub fn ino(&self) -> u32 {
        self.inode_index.ino
    }

    pub fn get_inode_index(&self) -> Index {
        self.inode_index
    }

    pub fn get_inode(&self) -> Arc<dyn InodeOps> {
        let inode = self.inode.lock();
        match inode.upgrade() {
            None => {
                drop(inode);
                let inode = vfs().load_inode(self.sno(), self.ino()).expect("Failed to load inode");
                *self.inode.lock() = Arc::downgrade(&inode);
                inode
            }
            Some(inode) => inode,
        }
    }

    pub fn get_parent(&self) -> Option<Arc<Dentry>> {
        self.parent.clone()
    }

    pub fn lookup(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>> {
        if let Some(child) = self.children.lock().get(name)
            && let Some(child) = child.upgrade()
        {
            return Ok(child);
        }

        let lookup_ino = self.get_inode().lookup(name)?;
        let lookup_sno = self.sno();
        let inode = vfs().load_inode(lookup_sno, lookup_ino)?;

        let new_child = Arc::new(Self::new(name, self, &inode, lookup_sno));

        let mut children = self.children.lock();
        if let Some(existing_child) = children.get(name)
            && existing_child.upgrade().is_some()
        {
            Ok(existing_child.upgrade().unwrap())
        } else {
            children.insert(name.into(), Arc::downgrade(&new_child));
            Ok(new_child)
        }
    }

    pub fn lookup_nocached(self: &Arc<Self>, name: &str) -> SysResult<Arc<Dentry>> {
        let lookup_ino = self.get_inode().lookup(name)?;
        let lookup_sno = self.sno();
        let inode = vfs().load_inode(lookup_sno, lookup_ino)?;

        let new_child = Arc::new(Self::new(name, self, &inode, lookup_sno));

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
        if let Some(p) = self.parent.as_ref() {
            let inode = self.get_inode();
            let mut buffer = [0u8; 255];
            if let Some(length) = inode.readlink(&mut buffer)? {
                let link_name = core::str::from_utf8(&buffer[..length]).unwrap();
                let link_dentry = vfs().lookup_dentry(p, link_name)?;
                return Ok(link_dentry);
            }
        }
        Ok(self)
    }

    pub fn mount(self: &Arc<Self>, mount_to: &Arc<dyn InodeOps>, mount_to_sno: u32) {
        *self.mount_to.lock() = Some(Arc::new(Dentry {
            inode_index: Index {
                sno: mount_to_sno,
                ino: mount_to.get_ino(),
            },
            name: self.name.clone(),
            parent: self.parent.clone(),
            children: SpinLock::new(BTreeMap::new(), "Dentry::children"),
            inode: SpinLock::new(Arc::downgrade(mount_to), "Dentry::inode"),
            mount_to: SpinLock::new(None, "Dentry::mount_to"),
        }));
    }

    pub fn get_path(&self) -> String {
        if let Some(parent) = self.parent.as_ref() {
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

    pub fn create(self: &Arc<Self>, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        if self.lookup(name).is_ok() {
            return Err(Errno::EEXIST);
        }

        let inode = self.get_inode().create(name, mode, owner)?;
        vfs().cache.insert(
            &Index {
                sno: self.sno(),
                ino: inode.get_ino(),
            },
            inode.clone(),
        )?;

        Ok(inode)
    }

    pub fn unlink(self: &Arc<Self>, name: &str) -> SysResult<()> {
        let inode = self.get_inode();
        let inode_index = Index {
            sno: self.sno(),
            ino: inode.lookup(name)?,
        };

        inode.unlink(name)?;

        self.children.lock().remove(name);
        vfs().cache.remove(&inode_index);

        Ok(())
    }

    pub fn symlink(self: &Arc<Self>, target: &str) -> SysResult<()> {
        let inode = self.get_inode();

        inode.symlink(target)
    }

    pub fn link(self: &Arc<Self>, name: &str, target: &Arc<Dentry>) -> SysResult<()> {
        let target_inode = target.get_inode();
        self.get_inode().link(name, &target_inode)?;
        vfs().cache.insert(
            &Index {
                sno: self.sno(),
                ino: target_inode.get_ino(),
            },
            target_inode,
        )?;

        Ok(())
    }

    pub fn rename(self: &Arc<Self>, old_name: &str, new_parent: &Arc<Dentry>, new_name: &str) -> SysResult<()> {
        debug_assert!(self.sno() == new_parent.sno());
        debug_assert!(old_name != "." && old_name != "..");
        debug_assert!(new_name != "." && new_name != "..");
        if Arc::ptr_eq(self, new_parent) && old_name == new_name {
            return Ok(());
        }

        let old_parent_inode = self.get_inode();
        let old_ino = old_parent_inode.lookup(old_name)?;
        let new_parent_inode = new_parent.get_inode();
        let overwritten = match new_parent_inode.lookup(new_name) {
            Ok(ino) if ino != old_ino => Some(Index {
                sno: new_parent.sno(),
                ino,
            }),
            Ok(_) | Err(Errno::ENOENT) => None,
            Err(err) => return Err(err),
        };

        old_parent_inode.rename(old_name, &new_parent_inode, new_name)?;

        self.children.lock().remove(old_name);
        new_parent.children.lock().remove(new_name);
        if let Some(index) = overwritten {
            vfs().cache.remove(&index);
        }

        Ok(())
    }

    pub fn readlink(&self, child: &str, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let lookup_ino = self.get_inode().lookup(child)?;
        let lookup_sno = self.sno();
        let inode = vfs().load_inode(lookup_sno, lookup_ino)?;
        inode.readlink(buf)
    }
}

impl Debug for Dentry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Dentry {{ sno: {}, ino: {}, name: {} }}",
            self.sno(),
            self.ino(),
            self.name
        )
    }
}
