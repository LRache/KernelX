use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use crate::kernel::errno::SysResult;
use crate::klib::SpinLock;

use super::{InodeOps, Index};

pub struct Cache {
    cache: SpinLock<BTreeMap<Index, Arc<dyn InodeOps>>>,
}

impl Cache {
    pub const fn new() -> Self {
        Self {
            cache: SpinLock::new(BTreeMap::new(), "InodeCache::cache"),
        }
    }

    // pub fn find(&self, index: &Index) -> Option<Arc<dyn InodeOps>> {
    //     self.cache.lock().get(index).cloned()
    // }

    // pub fn insert(&self, index: &Index, inode: Arc<dyn InodeOps>) -> SysResult<()> {
    //     let mut cache = self.cache.lock();
        
    //     if cache.len() >= config::INODE_CACHE_SIZE {
    //         cache.retain(|_, inode| {
    //             Arc::strong_count(inode) > 1
    //         });
    //         let final_size = cache.len();

    //         if final_size >= config::INODE_CACHE_SIZE {
    //             return Err(Errno::ENOSPC);
    //         }
    //     }
        
    //     cache.insert(*index, inode);
        
    //     Ok(())
    // }

    pub fn sync(&self) -> SysResult<()> {
        let cache = self.cache.lock();
        for (_, inode) in cache.iter() {
            inode.sync()?;
        }
        Ok(())
    }
}