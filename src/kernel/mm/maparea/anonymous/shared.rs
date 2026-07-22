use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::PageTable;
use crate::kernel::mm::maparea::nofilemap::{AnonMapFamilyRegistration, SwappablePageFrame};
use crate::kernel::mm::maparea::{Area, MapAreaInfo, MapChangeNotifier, MemoryFaultSignal, PinPageFrame};
use crate::kernel::mm::swappable::AccessDirty;
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType, PhysPageFrame};
use crate::klib::{SleepLock, SpinLock};

enum SharedFrameState {
    Unallocated,
    Allocated(Arc<SwappablePageFrame>),
}

struct SharedAnonymousObject {
    frames: Vec<SleepLock<SharedFrameState>>,
}

impl SharedAnonymousObject {
    fn new(page_count: usize) -> Self {
        Self {
            frames: Vec::from_iter(
                (0..page_count).map(|_| SleepLock::new(SharedFrameState::Unallocated, "SharedAnonymous::frame")),
            ),
        }
    }
}

impl Drop for SharedAnonymousObject {
    fn drop(&mut self) {
        for frame in &self.frames {
            let page = {
                let mut state = frame.lock();
                match core::mem::replace(&mut *state, SharedFrameState::Unallocated) {
                    SharedFrameState::Allocated(page) => Some(page),
                    SharedFrameState::Unallocated => None,
                }
            };
            if let Some(page) = page {
                page.release_mapping_ref();
            }
        }
    }
}

pub struct SharedAnonymousArea {
    ubase: usize,
    perm: MapPerm,
    object: Arc<SharedAnonymousObject>,
    page_base: usize,
    page_count: usize,
    family_registration: AnonMapFamilyRegistration,
}

impl SharedAnonymousArea {
    pub fn new(ubase: usize, perm: MapPerm, page_count: usize) -> Self {
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");

        Self {
            ubase,
            perm,
            object: Arc::new(SharedAnonymousObject::new(page_count)),
            page_base: 0,
            page_count,
            family_registration: AnonMapFamilyRegistration::new_root(),
        }
    }

    fn object_page_index(&self, frame_index: usize) -> Option<usize> {
        if frame_index >= self.page_count {
            return None;
        }
        self.page_base.checked_add(frame_index)
    }

    fn materialize_page(&self, frame_index: usize) -> Option<(Arc<SwappablePageFrame>, bool)> {
        let page_index = self.object_page_index(frame_index)?;
        let frame = self.object.frames.get(page_index)?;
        let mut state = frame.lock();

        match &*state {
            SharedFrameState::Allocated(page) => Some((page.clone(), false)),
            SharedFrameState::Unallocated => {
                let page = SwappablePageFrame::new_mapped(
                    self.family_registration.page_backend(page_index),
                    PhysPageFrame::alloc_with_shrink_zeroed(),
                );
                *state = SharedFrameState::Allocated(page.clone());
                Some((page, true))
            }
        }
    }

    fn page(&self, frame_index: usize) -> Option<Arc<SwappablePageFrame>> {
        let page_index = self.object_page_index(frame_index)?;
        let state = self.object.frames.get(page_index)?.lock();
        match &*state {
            SharedFrameState::Allocated(page) => Some(page.clone()),
            SharedFrameState::Unallocated => None,
        }
    }

    fn translate(&self, uaddr: usize, addrspace: &AddrSpace, dirty: bool) -> Option<PinPageFrame> {
        let area_offset = uaddr.checked_sub(self.ubase)?;
        let frame_index = area_offset / arch::PGSIZE;
        let (page, newly_allocated) = self.materialize_page(frame_index)?;

        let mut guard = page.ensure_page().ok()?;
        if dirty {
            guard.mark_dirty();
        }
        if newly_allocated {
            let page_uaddr = self.ubase.checked_add(frame_index.checked_mul(arch::PGSIZE)?)?;
            addrspace.mmap_swappable(page_uaddr, &guard, self.perm);
        }
        drop(guard);
        let pin = page.pin_page(dirty).ok()?;
        Some(PinPageFrame::swappable(pin))
    }
}

impl Area for SharedAnonymousArea {
    fn bind_addrspace(&mut self, addrspace: &Arc<AddrSpace>) {
        if !self.family_registration.is_bound() {
            self.family_registration
                .bind(addrspace, self.ubase, self.page_base, self.page_count);
        }
    }

    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<PinPageFrame> {
        self.translate(uaddr, addrspace, false)
    }

    fn translate_write(
        &mut self,
        uaddr: usize,
        addrspace: &AddrSpace,
        _map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<PinPageFrame> {
        if !self.perm.contains(MapPerm::W) {
            return None;
        }
        self.translate(uaddr, addrspace, true)
    }

    fn get_frame(
        &mut self,
        uaddr: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<PinPageFrame> {
        self.translate_write(uaddr, addrspace, map_change_notifier)
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_ubase(&mut self, ubase: usize) {
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        self.ubase = ubase;

        if self.family_registration.is_bound() {
            self.family_registration
                .update(self.ubase, self.page_base, self.page_count);
        }
    }

    fn page_count(&self) -> usize {
        self.page_count
    }

    fn fork(
        &mut self,
        _self_pagetable: &SpinLock<PageTable>,
        _tlb_changed: &mut bool,
        addrspace: &Arc<AddrSpace>,
    ) -> Box<dyn Area> {
        Box::new(SharedAnonymousArea {
            ubase: self.ubase,
            perm: self.perm,
            object: self.object.clone(),
            page_base: self.page_base,
            page_count: self.page_count,
            family_registration: self
                .family_registration
                .fork(addrspace, self.ubase, self.page_base, self.page_count),
        })
    }

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        _access_type: MemAccessType,
        addrspace: &AddrSpace,
        _map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Result<(), MemoryFaultSignal> {
        let area_offset = uaddr.checked_sub(self.ubase).ok_or(MemoryFaultSignal::Segv)?;
        let frame_index = area_offset / arch::PGSIZE;
        let (page, _) = self.materialize_page(frame_index).ok_or(MemoryFaultSignal::Segv)?;
        let guard = page.ensure_page().map_err(|_| MemoryFaultSignal::Bus)?;
        let page_uaddr = self
            .ubase
            .checked_add(frame_index.checked_mul(arch::PGSIZE).ok_or(MemoryFaultSignal::Segv)?)
            .ok_or(MemoryFaultSignal::Segv)?;
        addrspace.mmap_replace_swappable(page_uaddr, &guard, self.perm);
        Ok(())
    }

    fn split(mut self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(
            uaddr > self.ubase && uaddr < self.ubase + self.size(),
            "uaddr out of range for split"
        );

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let right_page_base = self
            .page_base
            .checked_add(split_index)
            .expect("shared anonymous split page index overflow");
        let right_page_count = self.page_count - split_index;
        let right_family_registration = self.family_registration.split(
            self.ubase,
            self.page_base,
            split_index,
            uaddr,
            right_page_base,
            right_page_count,
        );

        self.page_count = split_index;
        let right = SharedAnonymousArea {
            ubase: uaddr,
            perm: self.perm,
            object: self.object.clone(),
            page_base: right_page_base,
            page_count: right_page_count,
            family_registration: right_family_registration,
        };

        (self, Box::new(right))
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>, tlb_changed: &mut bool) {
        if perm == self.perm {
            return;
        }

        self.perm = perm;
        for frame_index in 0..self.page_count {
            let Some(page) = self.page(frame_index) else {
                continue;
            };
            let uaddr = self.ubase + frame_index * arch::PGSIZE;
            page.with_resident_and_record_ad(false, |frame| {
                let access_dirty =
                    pagetable
                        .lock()
                        .mmap_replace_perm_with_check_and_ad_no_flush(uaddr, frame.get_page(), perm);
                *tlb_changed |= access_dirty.is_some();
                let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                ((), AccessDirty { accessed, dirty })
            });
        }
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        self.family_registration.with_tlb_invalidation_batch(|token| {
            let mut tlb_changed = false;
            for frame_index in 0..self.page_count {
                let Some(page) = self.page(frame_index) else {
                    continue;
                };
                let uaddr = self.ubase + frame_index * arch::PGSIZE;
                page.begin_tlb_invalidation(token, |frame| {
                    let access_dirty = pagetable
                        .lock()
                        .munmap_with_check_and_ad_no_flush(uaddr, frame.get_page());
                    tlb_changed |= access_dirty.is_some();
                    access_dirty.map(AccessDirty::from)
                });
            }
            if tlb_changed {
                arch::flush_tlb_all();
            }
            for frame_index in 0..self.page_count {
                if let Some(page) = self.page(frame_index) {
                    page.finish_tlb_invalidation(token);
                }
            }
        });
        self.family_registration.unregister();
    }

    fn type_name(&self) -> &'static str {
        "shared-anonymous"
    }

    fn map_area_info(&self) -> MapAreaInfo {
        let mut info = MapAreaInfo::new(self.ubase(), self.ubase() + self.size(), self.perm);
        info.shared = true;
        info
    }
}

impl Drop for SharedAnonymousArea {
    fn drop(&mut self) {
        self.family_registration.unregister();
    }
}
