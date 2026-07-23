use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::config;
use crate::kernel::errno::SysResult;
use crate::klib::SpinLock;
use crate::klib::lru::LRUCache;

use super::{Index, Inode};

pub struct Cache {
    cache: SpinLock<LRUCache<Index, Arc<Inode>>>,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            cache: SpinLock::new(LRUCache::new(), "InodeCache::cache"),
        }
    }

    fn is_idle(inode: &Arc<Inode>) -> bool {
        Arc::strong_count(inode) <= 1
    }

    fn low_watermark() -> usize {
        config::INODE_CACHE_LOW_WATERMARK.min(config::INODE_CACHE_HIGH_WATERMARK.saturating_sub(1))
    }

    fn remove_entry(cache: &mut LRUCache<Index, Arc<Inode>>, index: &Index) -> Option<Arc<Inode>> {
        let inode = cache.get(index).cloned();
        if inode.is_some() {
            cache.remove(index);
        }
        inode
    }

    fn remove_if(&self, mut predicate: impl FnMut(&Index, &Arc<Inode>) -> bool) -> usize {
        let removed = {
            let mut cache = self.cache.lock();
            let mut reclaimable = Vec::new();
            let _ = cache.try_for_each_mut(|index, inode| -> Result<(), ()> {
                if predicate(&index, inode) {
                    reclaimable.push(index);
                }
                Ok(())
            });
            reclaimable
                .into_iter()
                .filter_map(|index| Self::remove_entry(&mut cache, &index))
                .collect::<Vec<_>>()
        };

        let count = removed.len();
        drop(removed);
        count
    }

    // Return removed inodes so Drop runs after InodeCache::cache is unlocked.
    fn reclaim(cache: &mut LRUCache<Index, Arc<Inode>>) -> Vec<Arc<Inode>> {
        let mut removed = Vec::new();
        let mut remaining_to_scan = cache.len();
        let low_watermark = Self::low_watermark();

        while cache.len() > low_watermark && remaining_to_scan > 0 {
            remaining_to_scan -= 1;
            let Some((index, idle)) = cache.tail().map(|(index, inode)| (index, Self::is_idle(inode))) else {
                break;
            };

            if idle {
                let Some(inode) = Self::remove_entry(cache, &index) else {
                    continue;
                };
                removed.push(inode);
            } else {
                cache.access(&index);
            }
        }

        removed
    }

    pub fn find(&self, index: &Index) -> Option<Arc<Inode>> {
        self.cache.lock().get(index).cloned()
    }

    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    pub fn get_or_insert(&self, index: &Index, inode: Arc<Inode>) -> SysResult<Arc<Inode>> {
        let (inode, removed) = {
            let mut cache = self.cache.lock();
            let mut removed = Vec::new();

            // Recheck while holding the cache lock because the inode was loaded without it.
            if cache.contains_key(index) {
                let existing = cache
                    .get(index)
                    .cloned()
                    .expect("inode disappeared while cache is locked");
                removed.push(inode);
                (existing, removed)
            } else {
                if cache.len() >= config::INODE_CACHE_HIGH_WATERMARK {
                    removed = Self::reclaim(&mut cache);
                }

                cache.put(*index, inode.clone());
                (inode, removed)
            }
        };
        drop(removed);
        Ok(inode)
    }

    pub fn remove(&self, index: &Index) {
        let removed = {
            let mut cache = self.cache.lock();
            Self::remove_entry(&mut cache, index)
        };
        drop(removed);
    }

    pub fn clear(&self) {
        let removed = {
            let mut cache = self.cache.lock();
            cache.drain()
        };
        drop(removed);
    }

    pub fn prune_unused(&self) -> usize {
        let removed = {
            let mut cache = self.cache.lock();
            if cache.len() <= config::INODE_CACHE_LOW_WATERMARK {
                Vec::new()
            } else {
                Self::reclaim(&mut cache)
            }
        };

        let count = removed.len();
        drop(removed);
        count
    }

    pub fn remove_superblock(&self, sno: u32) -> usize {
        self.remove_if(|index, _| index.sno == sno)
    }

    pub fn superblock_busy(&self, sno: u32) -> bool {
        self.cache
            .lock()
            .try_for_each_mut(|index, inode| -> Result<(), ()> {
                if index.sno == sno && !Self::is_idle(inode) {
                    return Err(());
                }
                Ok(())
            })
            .is_err()
    }

    pub fn sync(&self) -> SysResult<()> {
        let inodes = {
            let mut cache = self.cache.lock();
            let mut inodes = Vec::new();
            let _ = cache.try_for_each_mut(|_, inode| -> Result<(), ()> {
                inodes.push(inode.clone());
                Ok(())
            });
            inodes
        };
        for inode in inodes {
            inode.sync()?;
        }
        Ok(())
    }
}
