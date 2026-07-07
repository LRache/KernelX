use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use crate::arch::{self, KvmPageTable, PageTableTrait};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::{Manager, MapChange, MapChangeEvent, MapManagerWatcher};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::klib::{SleepLock, SpinLock};

struct KvmUserMemorySlot {
    owner: Weak<AddrSpace>,
    ubase: usize,
    gbase: usize,
    page_count: usize,
}

impl KvmUserMemorySlot {
    fn end(&self) -> Option<usize> {
        self.page_count
            .checked_mul(arch::PGSIZE)
            .and_then(|size| self.ubase.checked_add(size))
    }

    fn guest_end(&self) -> Option<usize> {
        self.page_count
            .checked_mul(arch::PGSIZE)
            .and_then(|size| self.gbase.checked_add(size))
    }

    fn user_range_overlaps(&self, start: usize, end: usize) -> bool {
        let Some(slot_end) = self.end() else {
            return true;
        };
        start < slot_end && self.ubase < end
    }

    fn contains_guest_addr(&self, gaddr: usize) -> bool {
        let Some(slot_end) = self.guest_end() else {
            return gaddr >= self.gbase;
        };
        self.gbase <= gaddr && gaddr < slot_end
    }
}

struct KvmUserMemoryFault {
    owner: Arc<AddrSpace>,
    uaddr: usize,
    gpage: usize,
}

impl KvmUserMemoryFault {
    fn stage2_perm(access_type: MemAccessType) -> MapPerm {
        match access_type {
            MemAccessType::Read => MapPerm::R,
            MemAccessType::Write => MapPerm::R | MapPerm::W,
            MemAccessType::Execute => MapPerm::X,
        }
    }

    fn translate(&self, map_manager: &mut Manager, access_type: MemAccessType) -> Option<(usize, MapPerm)> {
        let kaddr = match access_type {
            MemAccessType::Read | MemAccessType::Execute => map_manager.translate_read(self.uaddr, &self.owner)?,
            MemAccessType::Write => map_manager.translate_write(self.uaddr, &self.owner)?,
        };

        Some((kaddr & !arch::PGMASK, Self::stage2_perm(access_type)))
    }
}

struct KvmMapManager {
    user_slots: BTreeMap<usize, KvmUserMemorySlot>,
}

impl KvmMapManager {
    fn new() -> Self {
        Self {
            user_slots: BTreeMap::new(),
        }
    }

    fn is_user_range_overlapped(&self, start: usize, page_count: usize) -> bool {
        let Some(size) = page_count.checked_mul(arch::PGSIZE) else {
            return true;
        };
        let Some(end) = start.checked_add(size) else {
            return true;
        };

        self.user_slots
            .values()
            .any(|slot| slot.user_range_overlaps(start, end))
    }

    fn watch_user_memory(
        &mut self,
        owner: Weak<AddrSpace>,
        ubase: usize,
        gbase: usize,
        page_count: usize,
    ) -> SysResult<()> {
        if ubase % arch::PGSIZE != 0 || gbase % arch::PGSIZE != 0 || page_count == 0 {
            return Err(Errno::EINVAL);
        }
        if self.is_user_range_overlapped(ubase, page_count) {
            return Err(Errno::EINVAL);
        }

        self.user_slots.insert(
            ubase,
            KvmUserMemorySlot {
                owner,
                ubase,
                gbase,
                page_count,
            },
        );
        Ok(())
    }

    fn unwatch_all_user_memory(&mut self, pagetable: &SpinLock<KvmPageTable>) -> Vec<Arc<AddrSpace>> {
        let mut owners = Vec::new();

        for slot in self.user_slots.values() {
            if let Some(owner) = slot.owner.upgrade()
                && !owners.iter().any(|registered| Arc::ptr_eq(registered, &owner))
            {
                owners.push(owner);
            }
        }

        let mut pagetable = pagetable.lock();
        for slot in self.user_slots.values() {
            for page_index in 0..slot.page_count {
                let Some(offset) = page_index.checked_mul(arch::PGSIZE) else {
                    break;
                };
                let Some(gpage) = slot.gbase.checked_add(offset) else {
                    break;
                };
                let _ = pagetable.munmap_if_mapped(gpage);
            }
        }
        drop(pagetable);

        self.user_slots.clear();
        owners
    }

    fn invalidate_user_range(&mut self, uaddr: usize, page_count: usize, pagetable: &SpinLock<KvmPageTable>) {
        let Some(size) = page_count.checked_mul(arch::PGSIZE) else {
            return;
        };
        let Some(end) = uaddr.checked_add(size) else {
            return;
        };

        let mut pagetable = pagetable.lock();
        for slot in self.user_slots.values_mut() {
            let Some(slot_end) = slot.end() else {
                continue;
            };
            let overlap_start = core::cmp::max(uaddr, slot.ubase);
            let overlap_end = core::cmp::min(end, slot_end);
            if overlap_start >= overlap_end {
                continue;
            }

            let first_page = (overlap_start - slot.ubase) / arch::PGSIZE;
            let last_page = (overlap_end - slot.ubase + arch::PGSIZE - 1) / arch::PGSIZE;
            for page_index in first_page..last_page {
                let gpage = slot.gbase + page_index * arch::PGSIZE;
                let _ = pagetable.munmap_if_mapped(gpage);
            }
        }
    }

    fn find_user_memory_fault(&self, gaddr: usize) -> Option<KvmUserMemoryFault> {
        let slot = self
            .user_slots
            .values()
            .filter(|slot| slot.contains_guest_addr(gaddr))
            .max_by_key(|slot| slot.gbase)?;
        let owner = slot.owner.upgrade()?;
        let page_index = (gaddr - slot.gbase) / arch::PGSIZE;

        Some(KvmUserMemoryFault {
            owner,
            uaddr: slot.ubase + page_index * arch::PGSIZE,
            gpage: slot.gbase + page_index * arch::PGSIZE,
        })
    }
}

pub struct KvmAddrSpace {
    map_manager: SleepLock<KvmMapManager>,
    pagetable: SpinLock<KvmPageTable>,
}

impl KvmAddrSpace {
    pub fn new() -> Arc<Self> {
        let mut pagetable = KvmPageTable::new();
        pagetable.create();

        Arc::new(Self {
            map_manager: SleepLock::new(KvmMapManager::new(), "KvmAddrSpace::map_manager"),
            pagetable: SpinLock::new(pagetable, "KvmAddrSpace::pagetable"),
        })
    }

    pub fn pagetable(&self) -> &SpinLock<KvmPageTable> {
        &self.pagetable
    }

    pub fn watch_user_memory(
        self: &Arc<Self>,
        owner: &Arc<AddrSpace>,
        ubase: usize,
        gbase: usize,
        page_count: usize,
    ) -> SysResult<()> {
        self.map_manager
            .lock()
            .watch_user_memory(Arc::downgrade(owner), ubase, gbase, page_count)?;

        let watcher: Arc<dyn MapManagerWatcher> = self.clone();
        owner.add_map_manager_watcher(watcher);
        Ok(())
    }

    pub fn invalidate_user_range(&self, uaddr: usize, page_count: usize) {
        self.map_manager
            .lock()
            .invalidate_user_range(uaddr, page_count, &self.pagetable);
    }

    pub fn unwatch_all_user_memory(self: &Arc<Self>) {
        let owners = self.map_manager.lock().unwatch_all_user_memory(&self.pagetable);
        let watcher: Arc<dyn MapManagerWatcher> = self.clone();
        for owner in owners {
            owner.remove_map_manager_watcher(&watcher);
        }
    }

    pub fn try_to_fix_memory_fault(&self, gaddr: usize, access_type: MemAccessType) -> bool {
        let user_fault = {
            let map_manager = self.map_manager.lock();
            map_manager.find_user_memory_fault(gaddr)
        };

        let Some(user_fault) = user_fault else {
            return false;
        };

        user_fault
            .owner
            .with_map_manager_mut(|map_manager| {
                let (kpage, requested_perm) = user_fault.translate(map_manager, access_type)?;
                let mut pagetable = self.pagetable.lock();
                let perm = pagetable
                    .mapped_perm(user_fault.gpage)
                    .map(|current_perm| current_perm | requested_perm)
                    .unwrap_or(requested_perm);

                if pagetable.is_mapped(user_fault.gpage) {
                    // SAFETY: The owner MapManager lock is held across host
                    // translation and this G-stage PTE update. Host unmap/remap/COW
                    // paths notify KVM while holding the same lock before replacing
                    // or releasing the backing page, so they cannot miss this PTE.
                    unsafe { pagetable.mmap_replace_raw(user_fault.gpage, kpage, perm) };
                } else {
                    // SAFETY: The owner MapManager lock is held across host
                    // translation and this G-stage PTE update. Host unmap/remap/COW
                    // paths notify KVM while holding the same lock before replacing
                    // or releasing the backing page, so they cannot miss this PTE.
                    unsafe { pagetable.mmap_raw(user_fault.gpage, kpage, perm) };
                }

                Some(())
            })
            .is_some()
    }
}

impl MapManagerWatcher for KvmAddrSpace {
    fn before_map_change(&self, change: MapChange) {
        match change.event {
            MapChangeEvent::Unmap | MapChangeEvent::Remap => {
                self.invalidate_user_range(change.uaddr, change.page_count);
            }
            MapChangeEvent::PermChange(perm) => {
                let _ = perm;
                self.invalidate_user_range(change.uaddr, change.page_count);
            }
        }
    }
}
