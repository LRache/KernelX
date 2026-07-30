use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::kernel::mm::maparea::nofilemap::{AnonMapFamilyRegistration, FrameState, SwappablePageFrame};
use crate::kernel::mm::maparea::{Area, MapChange, MapChangeEvent, MapChangeNotifier, MemoryFaultError, PinPageFrame};
use crate::kernel::mm::swappable::AccessDirty;
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType, PhysPageFrame};
use crate::klib::SpinLock;

use super::super::slots::AreaPageSlots;

pub struct PrivateAnonymousArea {
    ubase: usize,
    perm: MapPerm,
    frames: AreaPageSlots<FrameState>,

    /* For swappable page */
    family_registration: AnonMapFamilyRegistration,
    page_base: usize,
}

impl PrivateAnonymousArea {
    pub fn new(ubase: usize, perm: MapPerm, page_count: usize) -> Self {
        // Anonymous areas should be page-aligned
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");

        Self {
            ubase,
            perm,
            frames: AreaPageSlots::new(page_count),
            family_registration: AnonMapFamilyRegistration::new_root(),
            page_base: 0,
        }
    }

    fn allocate_page(&mut self, frame_index: usize, addrspace: &AddrSpace, dirty: bool) -> usize {
        debug_assert!(frame_index < self.frames.len());
        debug_assert!(self.frames.get(frame_index).is_none());

        let page_index = self
            .page_base
            .checked_add(frame_index)
            .expect("anonymous page index overflow");
        let page = SwappablePageFrame::new_mapped(
            self.family_registration.page_backend(page_index),
            PhysPageFrame::alloc_with_shrink_zeroed(),
        );
        self.frames.insert(frame_index, FrameState::Allocated(page.clone()));

        let mut guard = page
            .ensure_page()
            .expect("new anonymous swappable page must be resident");
        if dirty {
            guard.mark_dirty();
        }
        let uaddr = self.ubase + frame_index * arch::PGSIZE;
        addrspace.mmap_swappable(uaddr, &guard, self.perm);
        guard.frame().get_page()
    }

    fn copy_on_write_page(
        &mut self,
        frame_index: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<()> {
        debug_assert!(frame_index < self.frames.len());
        debug_assert!(self.frames.get(frame_index).is_some_and(FrameState::is_cow));

        let old_page = match self.frames.get(frame_index)? {
            FrameState::Cow(page) => page.clone(),
            FrameState::Allocated(_) => unreachable!(),
        };
        let uaddr = self.ubase + frame_index * arch::PGSIZE;
        map_change_notifier.before_map_change(MapChange {
            uaddr,
            page_count: 1,
            event: MapChangeEvent::Remap,
        });

        if old_page.mapping_refs() == 1 {
            // The page is only mapped in this address space, we can just remap it as writable.
            let mut guard = old_page.ensure_page().ok()?;
            addrspace.mmap_replace_swappable_if_maps(uaddr, &guard, &guard, self.perm)?;
            guard.mark_dirty();
            drop(guard);
            self.frames.insert(frame_index, FrameState::Allocated(old_page));
            return Some(());
        }

        // Allocate before taking the old page lock: allocation may invoke
        // reclaim, which must never select this page and wait on a lock
        // held by the allocating task itself.
        let new_frame = PhysPageFrame::alloc_with_shrink_zeroed();
        let mut old_guard = old_page.ensure_page().ok()?;
        // Copy the content BEFORE installing the new frame into the PTE,
        // otherwise a concurrent fault on `uaddr` from another task/CPU
        // could observe the zeroed page between the PTE replacement and
        // the copy.
        new_frame.slice().copy_from_slice(old_guard.frame().slice());
        let page_index = self
            .page_base
            .checked_add(frame_index)
            .expect("anonymous page index overflow");
        let new_page = SwappablePageFrame::new_mapped(self.family_registration.page_backend(page_index), new_frame);
        let Ok(mut new_guard) = new_page.ensure_page() else {
            new_page.release_mapping_ref();
            return None;
        };
        new_guard.mark_dirty();

        let Some(AccessDirty { dirty: old_dirty, .. }) =
            addrspace.mmap_replace_swappable_if_maps(uaddr, &old_guard, &new_guard, self.perm)
        else {
            // The page may have been unmapped by another task,
            // we just release the new page and return None.
            new_guard.release_mapping_ref();
            return None;
        };
        if old_dirty {
            old_guard.mark_dirty();
        }
        self.frames.insert(frame_index, FrameState::Allocated(new_page.clone()));

        old_guard.release_mapping_ref();

        Some(())
    }

    fn map_swappable_page(
        &self,
        frame_index: usize,
        page: &Arc<SwappablePageFrame>,
        addrspace: &AddrSpace,
        perm: MapPerm,
    ) -> Option<usize> {
        let guard = page.ensure_page().ok()?;
        let uaddr = self.ubase + frame_index * arch::PGSIZE;
        addrspace.mmap_replace_swappable(uaddr, &guard, perm);
        Some(guard.frame().get_page())
    }
}

impl Area for PrivateAnonymousArea {
    fn bind_addrspace(&mut self, addrspace: &Arc<AddrSpace>) {
        if !self.family_registration.is_bound() {
            self.family_registration
                .bind(addrspace, self.ubase, self.page_base, self.frames.len());
        }
    }

    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<PinPageFrame> {
        debug_assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index >= self.frames.len() {
            return None;
        }
        if self.frames.get(page_index).is_none() {
            self.allocate_page(page_index, addrspace, false);
        }
        let page = match self.frames.get(page_index)? {
            FrameState::Allocated(page) | FrameState::Cow(page) => page.clone(),
        };
        let pin = page.pin_page(false).ok()?;
        Some(PinPageFrame::swappable(pin))
    }

    fn translate_write(
        &mut self,
        uaddr: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<PinPageFrame> {
        assert!(uaddr >= self.ubase);

        if !self.perm.contains(MapPerm::W) {
            return None;
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;

        if page_index >= self.frames.len() {
            return None;
        }

        if self.frames.get(page_index).is_none() {
            self.allocate_page(page_index, addrspace, true);
        } else if self.frames.get(page_index).is_some_and(FrameState::is_cow) {
            self.copy_on_write_page(page_index, addrspace, map_change_notifier)?;
        }
        let page = match self.frames.get(page_index)? {
            FrameState::Allocated(page) => page.clone(),
            FrameState::Cow(_) => unreachable!(),
        };
        let pin = page.pin_page(true).ok()?;
        Some(PinPageFrame::swappable(pin))
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

    fn fork(
        &mut self,
        self_pagetable: &SpinLock<PageTable>,
        tlb_cpu_mask: &mut usize,
        addrspace: &Arc<AddrSpace>,
    ) -> Box<dyn Area> {
        let cow_perm = self.perm - MapPerm::W;
        let mut child_frames = AreaPageSlots::new(self.frames.len());

        for (frame_index, state) in self.frames.iter_mut() {
            let child_state = match state {
                FrameState::Allocated(page) => {
                    let page = page.clone();
                    page.add_mapping_ref();
                    if self.perm.contains(MapPerm::W) {
                        let uaddr = self.ubase + frame_index * arch::PGSIZE;
                        page.with_resident_and_record_ad(false, |frame| {
                            let mut pagetable = self_pagetable.lock();
                            let access_dirty =
                                pagetable.mmap_replace_perm_with_check_and_ad(uaddr, frame.get_page(), cow_perm);
                            if access_dirty.is_some() {
                                *tlb_cpu_mask |= pagetable.active_cpu_mask();
                            }
                            let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                            ((), AccessDirty { accessed, dirty })
                        });
                    }
                    *state = FrameState::Cow(page.clone());
                    FrameState::Cow(page)
                }
                FrameState::Cow(page) => {
                    page.add_mapping_ref();
                    FrameState::Cow(page.clone())
                }
            };
            child_frames.insert(frame_index, child_state);
        }

        Box::new(Self {
            ubase: self.ubase,
            perm: self.perm,
            frames: child_frames,
            family_registration: self.family_registration.fork(
                addrspace,
                self.ubase,
                self.page_base,
                self.frames.len(),
            ),
            page_base: self.page_base,
        })
    }

    #[allow(unused_variables)]
    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        access_type: MemAccessType,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Result<(), MemoryFaultError> {
        debug_assert!(uaddr >= self.ubase);

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index < self.frames.len() {
            match self.frames.get(page_index) {
                Some(FrameState::Allocated(frame)) => {
                    self.map_swappable_page(page_index, frame, addrspace, self.perm)
                        .ok_or(MemoryFaultError::BusAddressError)?;
                }
                Some(FrameState::Cow(frame)) => {
                    if access_type != MemAccessType::Write {
                        self.map_swappable_page(page_index, frame, addrspace, self.perm - MapPerm::W)
                            .ok_or(MemoryFaultError::BusAddressError)?;
                    } else {
                        self.copy_on_write_page(page_index, addrspace, map_change_notifier)
                            .ok_or(MemoryFaultError::BusAddressError)?;
                    }
                }
                None => {
                    self.allocate_page(page_index, addrspace, false);
                }
            }
            Ok(())
        } else {
            Err(MemoryFaultError::SegvMapError)
        }
    }

    fn set_ubase(&mut self, ubase: usize) {
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        self.ubase = ubase;

        if self.family_registration.is_bound() {
            self.family_registration
                .update(self.ubase, self.page_base, self.frames.len());
        }
    }

    fn page_count(&self) -> usize {
        self.frames.len()
    }

    fn split(mut self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(
            uaddr >= self.ubase && uaddr < self.ubase + self.size(),
            "uaddr out of range for split, urange: [{:#x}, {:#x}), uaddr: {:#x}",
            self.ubase,
            self.ubase + self.size(),
            uaddr
        );

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let new_ubase = self.ubase + split_index * arch::PGSIZE;

        let new_frames = self.frames.split_off(split_index);

        let right_page_base = self
            .page_base
            .checked_add(split_index)
            .expect("anonymous split page index overflow");
        let right_family_registration = self.family_registration.split(
            self.ubase,
            self.page_base,
            self.frames.len(),
            new_ubase,
            right_page_base,
            new_frames.len(),
        );

        let new_area = Self {
            ubase: new_ubase,
            perm: self.perm,
            frames: new_frames,
            family_registration: right_family_registration,
            page_base: right_page_base,
        };

        (self, Box::new(new_area))
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>, tlb_cpu_mask: &mut usize) {
        if perm == self.perm {
            return;
        }

        self.perm = perm;

        for (frame_index, state) in self.frames.iter() {
            let page = match state {
                FrameState::Allocated(page) | FrameState::Cow(page) => page,
            };
            let uaddr = self.ubase + frame_index * arch::PGSIZE;
            let frame_perm = if matches!(state, FrameState::Cow(_)) {
                perm - MapPerm::W
            } else {
                perm
            };
            page.with_resident_and_record_ad(false, |frame| {
                let mut pagetable = pagetable.lock();
                let access_dirty = pagetable.mmap_replace_perm_with_check_and_ad(uaddr, frame.get_page(), frame_perm);
                if access_dirty.is_some() {
                    *tlb_cpu_mask |= pagetable.active_cpu_mask();
                }
                let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                ((), AccessDirty { accessed, dirty })
            });
        }
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        self.family_registration.with_tlb_invalidation_batch(|token| {
            let mut tlb_cpu_mask = 0;
            for (frame_index, state) in self.frames.iter() {
                let page = match state {
                    FrameState::Allocated(page) | FrameState::Cow(page) => page,
                };
                let uaddr = self.ubase + frame_index * arch::PGSIZE;
                page.begin_tlb_invalidation(token, |frame| {
                    let mut pagetable = pagetable.lock();
                    let access_dirty = pagetable.munmap_with_check_and_ad(uaddr, frame.get_page());
                    if access_dirty.is_some() {
                        tlb_cpu_mask |= pagetable.active_cpu_mask();
                    }
                    access_dirty.map(AccessDirty::from)
                });
            }
            arch::flush_tlb_cpu_mask(tlb_cpu_mask);
            for (_, state) in self.frames.iter() {
                let page = match state {
                    FrameState::Allocated(page) | FrameState::Cow(page) => page,
                };
                assert!(page.finish_tlb_invalidation(token));
            }
        });

        self.frames.clear_with(|_, state| {
            let page = match state {
                FrameState::Allocated(page) | FrameState::Cow(page) => page,
            };
            page.release_mapping_ref();
        });
        self.family_registration.unregister();
    }

    fn type_name(&self) -> &'static str {
        "private-anonymous"
    }
}

impl Drop for PrivateAnonymousArea {
    fn drop(&mut self) {
        self.frames.clear_with(|_, state| {
            let page = match state {
                FrameState::Allocated(page) | FrameState::Cow(page) => page,
            };
            page.release_mapping_ref();
        });
        self.family_registration.unregister();
    }
}
