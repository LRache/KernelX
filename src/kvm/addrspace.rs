use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use crate::arch::{self, KvmPageTable};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::MemAccessType;
use crate::klib::{SleepLock, SpinLock};

use super::vmm::VMMapArea;

struct KvmMapManager {
    areas: BTreeMap<usize, Box<VMMapArea>>,
}

impl KvmMapManager {
    fn new() -> Self {
        Self { areas: BTreeMap::new() }
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

        false
    }

    fn map_area(&mut self, gaddr: usize, area: Box<VMMapArea>) -> SysResult<()> {
        if gaddr != area.gbase() || gaddr % arch::PGSIZE != 0 {
            return Err(Errno::EINVAL);
        }
        if self.is_map_range_overlapped(gaddr, area.page_count()) {
            return Err(Errno::EINVAL);
        }

        self.areas.insert(gaddr, area);
        Ok(())
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

    pub fn map_area(&self, gaddr: usize, area: Box<VMMapArea>) -> SysResult<()> {
        self.map_manager.lock().map_area(gaddr, area)
    }

    pub fn try_to_fix_memory_fault(&self, gaddr: usize, access_type: MemAccessType) -> Option<usize> {
        self.map_manager
            .lock()
            .try_to_fix_memory_fault(gaddr, access_type, &self.pagetable)
    }
}
