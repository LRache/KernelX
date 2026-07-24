use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;

use crate::arch::{self, PageTable, PageTableTrait};
use crate::kernel::ipc::shm::{ShmFrames, on_shm_area_drop};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::klib::SpinLock;

use super::{Area, MapAreaInfo, MapChangeNotifier, MemoryFaultSignal, PinPageFrame};

pub struct ShmArea {
    ubase: usize,
    frames: Arc<ShmFrames>,
    perm: MapPerm,
    shmid: usize,
}

impl ShmArea {
    pub fn new(ubase: usize, frames: Arc<ShmFrames>, perm: MapPerm, shmid: usize) -> Self {
        Self {
            ubase,
            frames,
            perm,
            shmid,
        }
    }
}

impl Drop for ShmArea {
    fn drop(&mut self) {
        on_shm_area_drop(self.shmid);
    }
}

impl Area for ShmArea {
    fn translate_read(&mut self, uaddr: usize, _addrspace: &AddrSpace) -> Option<PinPageFrame> {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let frames = self.frames.frames.lock();
        frames.get(page_index).cloned().map(PinPageFrame::stable)
    }

    fn translate_write(
        &mut self,
        uaddr: usize,
        _addrspace: &AddrSpace,
        _map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<PinPageFrame> {
        self.translate_read(uaddr, _addrspace)
    }

    fn get_frame(
        &mut self,
        uaddr: usize,
        _addrspace: &AddrSpace,
        _map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<PinPageFrame> {
        self.translate_write(uaddr, _addrspace, _map_change_notifier)
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn page_count(&self) -> usize {
        self.frames.frames.lock().len()
    }

    fn fork(
        &mut self,
        _self_pagetable: &SpinLock<PageTable>,
        _tlb_changed: &mut bool,
        _addrspace: &Arc<AddrSpace>,
    ) -> Box<dyn Area> {
        // Forked child gets its own ShmArea; increment ref_count to match.
        use crate::kernel::ipc::shm::on_shm_area_attach;
        on_shm_area_attach(self.shmid);
        Box::new(ShmArea {
            ubase: self.ubase,
            frames: self.frames.clone(),
            perm: self.perm,
            shmid: self.shmid,
        })
    }

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        _access_type: MemAccessType,
        addrspace: &AddrSpace,
        _map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Result<(), MemoryFaultSignal> {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let frames = self.frames.frames.lock();
        if page_index >= frames.len() {
            return Err(MemoryFaultSignal::Segv);
        }

        let frame = &frames[page_index];
        addrspace
            .pagetable()
            .lock()
            .mmap(uaddr & !arch::PGMASK, frame, self.perm);
        Ok(())
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let mut tlb_changed = false;
        let frames = self.frames.frames.lock();
        for i in 0..frames.len() {
            let uaddr = self.ubase + i * arch::PGSIZE;
            tlb_changed |= pagetable.lock().munmap_with_check_no_flush(uaddr, frames[i].get_page());
        }
        if tlb_changed {
            pagetable.lock().flush_tlb();
        }
    }

    fn type_name(&self) -> &'static str {
        "ShmArea"
    }

    // PERF_DEBUG(map-manager-lock-backing): Remove with Area::debug_backing_id.
    #[cfg(feature = "map-manager-lock-debug")]
    fn debug_backing_id(&self) -> usize {
        Arc::as_ptr(&self.frames) as usize
    }

    fn map_area_info(&self) -> MapAreaInfo {
        let mut info = MapAreaInfo::new(self.ubase(), self.ubase() + self.size(), self.perm);
        info.shared = true;
        info.path = Some(format!("[shm:{}]", self.shmid));
        info
    }
}
