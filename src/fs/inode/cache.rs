use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kinfo;

use super::{Index, InodeOps};

pub struct Cache {
    cache: Mutex<BTreeMap<Index, Arc<dyn InodeOps>>>,
}

impl Cache {
    pub const fn new() -> Self {
        Self {
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn find(&self, index: &Index) -> Option<Arc<dyn InodeOps>> {
        self.cache.lock().get(index).cloned()
    }

    pub fn insert(&self, index: &Index, inode: Arc<dyn InodeOps>) -> SysResult<()> {
        let mut cache = self.cache.lock();

        if cache.len() >= config::INODE_CACHE_SIZE {
            let initial_size = cache.len();
            cache.retain(|_, inode| Arc::strong_count(inode) > 1);
            let final_size = cache.len();
            kinfo!(
                "Uncached {} unused inodes, {} remaining",
                initial_size - final_size,
                final_size
            );

            if final_size >= config::INODE_CACHE_SIZE {
                return Err(Errno::ENOMEM);
            }
        }

        cache.insert(*index, inode);

        Ok(())
    }

    pub fn sync(&self) -> SysResult<()> {
        let cache = self.cache.lock();
        for (_, inode) in cache.iter() {
            inode.sync()?;
        }
        Ok(())
    }
}
