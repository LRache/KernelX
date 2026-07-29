use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;
use core::sync::atomic::{AtomicUsize, Ordering};

use smallvec::SmallVec;

use crate::kernel::config;
use crate::kernel::errno::SysResult;
use crate::kernel::event::Event;
use crate::kernel::scheduler::{self, Task, current};
use crate::klib::SpinLock;
use crate::klib::lru::LRUCache;

use super::{Index, Inode};

struct WaitQueue {
    state: SpinLock<WaitQueueState>,
}

struct WaitQueueState {
    completed: bool,
    waiters: SmallVec<[Arc<dyn Task>; 4]>,
}

impl WaitQueue {
    fn new() -> Self {
        Self {
            state: SpinLock::new(
                WaitQueueState {
                    completed: false,
                    waiters: SmallVec::new(),
                },
                "InodeCache::waiters",
            ),
        }
    }

    fn wait(&self) {
        let task = current::task().clone();
        {
            let mut state = self.state.lock();
            if state.completed {
                return;
            }
            scheduler::block_task_uninterruptible_named(
                task.clone(),
                "inode_cache_transition",
                "InodeCache::transition",
            );
            state.waiters.push(task);
        }
        current::schedule();
    }

    fn complete(&self) {
        let waiters = {
            let mut state = self.state.lock();
            state.completed = true;
            mem::take(&mut state.waiters)
        };
        for task in waiters {
            scheduler::wakeup_task_uninterruptible(task, Event::SleepLock);
        }
    }
}

enum CacheEntry {
    Loading(Arc<WaitQueue>),
    Ready(Arc<Inode>),
    Syncing {
        inode: Option<Arc<Inode>>,
        waiters: Arc<WaitQueue>,
    },
}

struct SyncWork {
    index: Index,
    inode: Arc<Inode>,
    waiters: Arc<WaitQueue>,
}

pub struct Cache {
    cache: SpinLock<LRUCache<Index, CacheEntry>>,
    background_reclaimed: AtomicUsize,
    foreground_reclaimed: AtomicUsize,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            cache: SpinLock::new(LRUCache::new(), "InodeCache::cache"),
            background_reclaimed: AtomicUsize::new(0),
            foreground_reclaimed: AtomicUsize::new(0),
        }
    }

    fn is_idle(entry: &CacheEntry) -> bool {
        matches!(entry, CacheEntry::Ready(inode) if Arc::strong_count(inode) <= 1)
    }

    fn low_watermark() -> usize {
        config::INODE_CACHE_LOW_WATERMARK.min(config::INODE_CACHE_HIGH_WATERMARK.saturating_sub(1))
    }

    fn start_reclaim(cache: &mut LRUCache<Index, CacheEntry>) -> Vec<SyncWork> {
        let mut works = Vec::new();
        let mut remaining_to_scan = cache.len();
        let low_watermark = Self::low_watermark();

        while cache.len().saturating_sub(works.len()) > low_watermark && remaining_to_scan > 0 {
            remaining_to_scan -= 1;
            let Some((index, inode)) = cache.tail().and_then(|(index, entry)| match entry {
                CacheEntry::Ready(inode) if Arc::strong_count(inode) <= 1 => Some((index, inode.clone())),
                _ => None,
            }) else {
                let Some((index, _)) = cache.tail() else {
                    break;
                };
                cache.access(&index);
                continue;
            };

            let waiters = Arc::new(WaitQueue::new());
            cache.put(
                index,
                CacheEntry::Syncing {
                    inode: Some(inode.clone()),
                    waiters: waiters.clone(),
                },
            );
            works.push(SyncWork { index, inode, waiters });
        }

        works
    }

    fn finish_reclaim(&self, works: Vec<SyncWork>) -> usize {
        let mut removed = 0;
        for work in works {
            let synced = work.inode.sync().is_ok();
            if synced {
                let cached_inode = {
                    let mut cache = self.cache.lock();
                    match cache.get_mut(&work.index) {
                        Some(CacheEntry::Syncing { inode, waiters }) if Arc::ptr_eq(waiters, &work.waiters) => {
                            inode.take().expect("syncing inode was already taken")
                        }
                        _ => unreachable!("inode cache syncing state disappeared"),
                    }
                };

                // Keep the Syncing marker published until the old inode has
                // completed its final Drop, so a loader cannot publish a new
                // inode for the same index too early.
                drop(work.inode);
                drop(cached_inode);

                let mut cache = self.cache.lock();
                let is_current = matches!(
                    cache.get(&work.index),
                    Some(CacheEntry::Syncing {
                        inode: None,
                        waiters,
                    }) if Arc::ptr_eq(waiters, &work.waiters)
                );
                if !is_current {
                    unreachable!("inode cache syncing state disappeared");
                }
                cache.remove(&work.index);
                removed += 1;
            } else {
                let mut cache = self.cache.lock();
                let is_current = matches!(
                    cache.get(&work.index),
                    Some(CacheEntry::Syncing { waiters, .. })
                        if Arc::ptr_eq(waiters, &work.waiters)
                );
                if !is_current {
                    unreachable!("inode cache syncing state disappeared");
                }
                cache.put(work.index, CacheEntry::Ready(work.inode.clone()));
            }
            work.waiters.complete();
        }
        removed
    }

    pub fn get_or_load(&self, index: &Index, load: impl FnOnce() -> SysResult<Arc<Inode>>) -> SysResult<Arc<Inode>> {
        loop {
            let (waiters, reclaim) = {
                let mut cache = self.cache.lock();
                match cache.get(index) {
                    Some(CacheEntry::Ready(inode)) => return Ok(inode.clone()),
                    Some(CacheEntry::Loading(waiters)) | Some(CacheEntry::Syncing { waiters, .. }) => {
                        (Some(waiters.clone()), Vec::new())
                    }
                    None => {
                        let reclaim = if cache.len() >= config::INODE_CACHE_HIGH_WATERMARK {
                            Self::start_reclaim(&mut cache)
                        } else {
                            Vec::new()
                        };
                        let waiters = Arc::new(WaitQueue::new());
                        cache.put(*index, CacheEntry::Loading(waiters.clone()));
                        (None, reclaim)
                    }
                }
            };

            if let Some(waiters) = waiters {
                waiters.wait();
                continue;
            }

            let reclaimed = self.finish_reclaim(reclaim);
            self.foreground_reclaimed.fetch_add(reclaimed, Ordering::Relaxed);
            let result = load();
            {
                let mut cache = self.cache.lock();
                let CacheEntry::Loading(waiters) = cache.get(index).expect("inode cache loading state disappeared")
                else {
                    unreachable!("inode cache loading state was replaced");
                };
                let waiters = waiters.clone();
                match &result {
                    Ok(inode) => cache.put(*index, CacheEntry::Ready(inode.clone())),
                    Err(_) => {
                        cache.remove(index);
                    }
                }
                drop(cache);
                waiters.complete();
            }
            return result;
        }
    }

    pub fn find_ready(&self, index: &Index) -> Option<Arc<Inode>> {
        let mut cache = self.cache.lock();
        match cache.get(index) {
            Some(CacheEntry::Ready(inode)) => Some(inode.clone()),
            Some(CacheEntry::Loading(_)) | Some(CacheEntry::Syncing { .. }) | None => None,
        }
    }

    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    pub fn get_or_insert(&self, index: &Index, inode: Arc<Inode>) -> SysResult<Arc<Inode>> {
        loop {
            let (existing, waiters, reclaim) = {
                let mut cache = self.cache.lock();
                match cache.get(index) {
                    Some(CacheEntry::Ready(existing)) => (Some(existing.clone()), None, Vec::new()),
                    Some(CacheEntry::Loading(waiters)) | Some(CacheEntry::Syncing { waiters, .. }) => {
                        (None, Some(waiters.clone()), Vec::new())
                    }
                    None => {
                        let reclaim = if cache.len() >= config::INODE_CACHE_HIGH_WATERMARK {
                            Self::start_reclaim(&mut cache)
                        } else {
                            Vec::new()
                        };
                        cache.put(*index, CacheEntry::Ready(inode.clone()));
                        (None, None, reclaim)
                    }
                }
            };

            if let Some(existing) = existing {
                return Ok(existing);
            }
            if let Some(waiters) = waiters {
                waiters.wait();
                continue;
            }

            let reclaimed = self.finish_reclaim(reclaim);
            self.foreground_reclaimed.fetch_add(reclaimed, Ordering::Relaxed);
            return Ok(inode);
        }
    }

    pub fn remove(&self, index: &Index) {
        loop {
            let (removed, waiters) = {
                let mut cache = self.cache.lock();
                match cache.get(index) {
                    Some(CacheEntry::Ready(inode)) => {
                        let inode = inode.clone();
                        cache.remove(index);
                        (Some(inode), None)
                    }
                    Some(CacheEntry::Loading(waiters)) | Some(CacheEntry::Syncing { waiters, .. }) => {
                        (None, Some(waiters.clone()))
                    }
                    None => return,
                }
            };
            drop(removed);
            let Some(waiters) = waiters else {
                return;
            };
            waiters.wait();
        }
    }

    pub fn clear(&self) {
        loop {
            let (removed, waiters) = {
                let mut cache = self.cache.lock();
                let mut waiters = Vec::new();
                let _ = cache.try_for_each_mut(|_, entry| -> Result<(), ()> {
                    match entry {
                        CacheEntry::Loading(queue) | CacheEntry::Syncing { waiters: queue, .. } => {
                            waiters.push(queue.clone());
                        }
                        CacheEntry::Ready(_) => {}
                    }
                    Ok(())
                });
                if waiters.is_empty() {
                    (cache.drain(), waiters)
                } else {
                    (Vec::new(), waiters)
                }
            };
            drop(removed);
            if waiters.is_empty() {
                return;
            }
            for waiters in waiters {
                waiters.wait();
            }
        }
    }

    pub fn prune_unused(&self) -> usize {
        let works = {
            let mut cache = self.cache.lock();
            if cache.len() < config::INODE_CACHE_HIGH_WATERMARK {
                Vec::new()
            } else {
                Self::start_reclaim(&mut cache)
            }
        };
        let reclaimed = self.finish_reclaim(works);
        self.background_reclaimed.fetch_add(reclaimed, Ordering::Relaxed);
        reclaimed
    }

    pub(in crate::fs) fn print_perf_info(&self) {
        crate::kinfo!("Inode Cache Reclaimer Performance Info:");
        crate::kinfo!(
            "  Reclaimed inodes: background={}, foreground_sync={}",
            self.background_reclaimed.load(Ordering::Relaxed),
            self.foreground_reclaimed.load(Ordering::Relaxed),
        );
    }

    pub fn remove_superblock(&self, sno: u32) -> usize {
        loop {
            let (removed, waiters) = {
                let mut cache = self.cache.lock();
                let mut indices = Vec::new();
                let mut waiters = Vec::new();
                let _ = cache.try_for_each_mut(|index, entry| -> Result<(), ()> {
                    if index.sno == sno {
                        match entry {
                            CacheEntry::Ready(_) => indices.push(index),
                            CacheEntry::Loading(queue) | CacheEntry::Syncing { waiters: queue, .. } => {
                                waiters.push(queue.clone());
                            }
                        }
                    }
                    Ok(())
                });
                if waiters.is_empty() {
                    let removed = indices
                        .into_iter()
                        .filter_map(|index| match cache.get(&index) {
                            Some(CacheEntry::Ready(inode)) => {
                                let inode = inode.clone();
                                cache.remove(&index);
                                Some(inode)
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    (removed, waiters)
                } else {
                    (Vec::new(), waiters)
                }
            };

            if waiters.is_empty() {
                let count = removed.len();
                drop(removed);
                return count;
            }
            for waiters in waiters {
                waiters.wait();
            }
        }
    }

    pub fn superblock_busy(&self, sno: u32) -> bool {
        self.cache
            .lock()
            .try_for_each_mut(|index, entry| -> Result<(), ()> {
                if index.sno == sno && !Self::is_idle(entry) {
                    return Err(());
                }
                Ok(())
            })
            .is_err()
    }

    pub fn sync(&self) -> SysResult<()> {
        let inodes = loop {
            let (inodes, waiters) = {
                let mut cache = self.cache.lock();
                let mut inodes = Vec::new();
                let mut waiters = Vec::new();
                let _ = cache.try_for_each_mut(|_, entry| -> Result<(), ()> {
                    match entry {
                        CacheEntry::Ready(inode) => inodes.push(inode.clone()),
                        CacheEntry::Loading(queue) | CacheEntry::Syncing { waiters: queue, .. } => {
                            waiters.push(queue.clone());
                        }
                    }
                    Ok(())
                });
                (inodes, waiters)
            };
            if waiters.is_empty() {
                break inodes;
            }
            for waiters in waiters {
                waiters.wait();
            }
        };
        for inode in inodes {
            inode.sync()?;
        }
        Ok(())
    }
}
