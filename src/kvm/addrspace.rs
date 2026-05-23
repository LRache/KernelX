use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use crate::arch::{self, KvmPageTable, PageTableTrait};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::{AddrSpace, AddrSpaceWatcher, MapPerm, MemAccessType};
use crate::klib::{SleepLock, SpinLock};

use super::vmm::VMMapArea;

struct KvmUserMemorySlot {
    ubase: usize,
    gbase: usize,
    page_count: usize,
    mapped_pages: BTreeMap<usize, usize>,
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

    fn guest_range_overlaps(&self, start: usize, end: usize) -> bool {
        let Some(slot_end) = self.guest_end() else {
            return true;
        };
        start < slot_end && self.gbase < end
    }
}

struct KvmMapManager {
    areas: BTreeMap<usize, VMMapArea>,
    user_slots: BTreeMap<usize, KvmUserMemorySlot>,
}

impl KvmMapManager {
    fn new() -> Self {
        Self {
            areas: BTreeMap::new(),
            user_slots: BTreeMap::new(),
        }
    }

    fn is_map_range_overlapped(&self, start: usize, page_count: usize) -> bool {
        let Some(size) = page_count.checked_mul(arch::PGSIZE) else {
            return true;
        };
        let Some(end) = start.checked_add(size) else {
            return true;
        };

        for (&area_base, area) in &self.areas {
            let area_end = area_base + area.size();
            if start < area_end && area_base < end {
                return true;
            }
        }

        for slot in self.user_slots.values() {
            if slot.guest_range_overlaps(start, end) {
                return true;
            }
        }

        false
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

    fn map_area(&mut self, gaddr: usize, area: VMMapArea) -> SysResult<()> {
        if gaddr != area.gbase() || gaddr % arch::PGSIZE != 0 {
            return Err(Errno::EINVAL);
        }
        if self.is_map_range_overlapped(gaddr, area.page_count()) {
            return Err(Errno::EINVAL);
        }

        self.areas.insert(gaddr, area);
        Ok(())
    }

    fn watch_user_memory(&mut self, ubase: usize, gbase: usize, page_count: usize) -> SysResult<()> {
        if ubase % arch::PGSIZE != 0 || gbase % arch::PGSIZE != 0 || page_count == 0 {
            return Err(Errno::EINVAL);
        }
        if self.is_map_range_overlapped(gbase, page_count) || self.is_user_range_overlapped(ubase, page_count) {
            return Err(Errno::EINVAL);
        }

        self.user_slots.insert(
            ubase,
            KvmUserMemorySlot {
                ubase,
                gbase,
                page_count,
                mapped_pages: BTreeMap::new(),
            },
        );
        Ok(())
    }

    fn record_user_page_mapping(&mut self, uaddr: usize, kpage: usize) -> Option<usize> {
        let (_ubase, slot) = self.user_slots.range_mut(..=uaddr).next_back()?;
        let slot_end = slot.end()?;
        if uaddr >= slot_end {
            return None;
        }

        let page_index = (uaddr - slot.ubase) / arch::PGSIZE;
        slot.mapped_pages.insert(page_index, kpage);
        Some(slot.gbase + page_index * arch::PGSIZE)
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
                let Some(kpage) = slot.mapped_pages.remove(&page_index) else {
                    continue;
                };
                let gpage = slot.gbase + page_index * arch::PGSIZE;
                let _ = pagetable.munmap_with_check(gpage, kpage);
            }
        }
    }

    fn try_to_fix_memory_fault(
        &mut self,
        gaddr: usize,
        access_type: MemAccessType,
        pagetable: &SpinLock<KvmPageTable>,
    ) -> Option<usize> {
        let Some((_gbase, area)) = self.areas.range_mut(..=gaddr).next_back() else {
            return None;
        };

        if let Some(kaddr) = area.try_to_fix_memory_fault(gaddr, access_type, pagetable) {
            Some(kaddr)
        } else {
            crate::kinfo!(
                "KVM area at {:#x} failed to fix memory fault at {:#x} for access type {:?}",
                area.gbase(),
                gaddr,
                access_type
            );
            None
        }
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

    pub fn is_map_range_overlapped(&self, start: usize, page_count: usize) -> bool {
        self.map_manager.lock().is_map_range_overlapped(start, page_count)
    }

    pub fn map_area(&self, gaddr: usize, area: VMMapArea) -> SysResult<()> {
        self.map_manager.lock().map_area(gaddr, area)
    }

    pub fn watch_user_memory(
        self: &Arc<Self>,
        owner: &AddrSpace,
        ubase: usize,
        gbase: usize,
        page_count: usize,
    ) -> SysResult<()> {
        self.map_manager.lock().watch_user_memory(ubase, gbase, page_count)?;

        let watcher: Arc<dyn AddrSpaceWatcher> = self.clone();
        owner.add_watcher(Arc::downgrade(&watcher));
        Ok(())
    }

    pub fn record_user_page_mapping(&self, uaddr: usize, kpage: usize) -> Option<usize> {
        self.map_manager.lock().record_user_page_mapping(uaddr, kpage)
    }

    pub fn invalidate_user_range(&self, uaddr: usize, page_count: usize) {
        self.map_manager
            .lock()
            .invalidate_user_range(uaddr, page_count, &self.pagetable);
    }

    pub fn try_to_fix_memory_fault(&self, gaddr: usize, access_type: MemAccessType) -> Option<usize> {
        self.map_manager
            .lock()
            .try_to_fix_memory_fault(gaddr, access_type, &self.pagetable)
    }
}

impl AddrSpaceWatcher for KvmAddrSpace {
    fn on_addrspace_unmap(&self, uaddr: usize, page_count: usize) {
        self.invalidate_user_range(uaddr, page_count);
    }

    fn on_addrspace_remap(&self, uaddr: usize, page_count: usize) {
        self.invalidate_user_range(uaddr, page_count);
    }

    fn on_addrspace_perm_change(&self, uaddr: usize, page_count: usize, _perm: MapPerm) {
        self.invalidate_user_range(uaddr, page_count);
    }
}
