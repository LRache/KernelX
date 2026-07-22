use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::arch;
use crate::arch::PageTable;
use crate::fs::file::{FileOps, RandomAccessFile};
use crate::kernel::mm::maparea::nofilemap::{AnonMapFamilyRegistration, FrameState, SwappablePageFrame as AreaFrame};
use crate::kernel::mm::maparea::{
    Area, MapAreaInfo, MapChange, MapChangeEvent, MapChangeNotifier, MemoryFaultSignal, PinPageFrame,
};
use crate::kernel::mm::swappable::AccessDirty;
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType, PhysPageFrame};
use crate::klib::SpinLock;

use super::super::slots::AreaPageSlots;

pub struct PrivateFileMapArea {
    ubase: usize,
    perm: MapPerm,

    file: Arc<RandomAccessFile>,
    file_offset: usize,
    file_length: usize,

    family_registration: AnonMapFamilyRegistration,
    page_base: usize,

    frames: AreaPageSlots<FrameState>,
}

impl PrivateFileMapArea {
    pub fn new(
        ubase: usize,
        perm: MapPerm,
        file: Arc<RandomAccessFile>,
        file_offset: usize,
        file_length: usize,
    ) -> Self {
        debug_assert!(ubase % arch::PGSIZE == 0, "ubase should be page-aligned");
        debug_assert!(file_offset % arch::PGSIZE == 0, "file_offset should be page-aligned");

        let page_count = (file_length + arch::PGSIZE - 1) / arch::PGSIZE;

        Self {
            ubase,
            perm,
            file,
            file_offset,
            file_length,
            family_registration: AnonMapFamilyRegistration::new_root(),
            page_base: 0,
            frames: AreaPageSlots::new(page_count),
        }
    }

    fn load_page(&mut self, page_index: usize, addrspace: &AddrSpace, dirty: bool) -> usize {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(self.frames.get(page_index).is_none());

        let area_offset = page_index
            .checked_mul(arch::PGSIZE)
            .expect("private file mapping page offset overflow");
        let file_offset = self
            .file_offset
            .checked_add(area_offset)
            .expect("private file mapping file offset overflow");
        let uaddr = self
            .ubase
            .checked_add(area_offset)
            .expect("private file mapping address overflow");

        let frame = PhysPageFrame::alloc_with_shrink_zeroed();

        if area_offset < self.file_length {
            let length = core::cmp::min(self.file_length - area_offset, arch::PGSIZE);
            if self.file.pread(&mut frame.slice()[..length], file_offset).is_err() {
                frame.slice().fill(0);
            }
        }

        let logical_page = self
            .page_base
            .checked_add(page_index)
            .expect("private file mapping anonymous page index overflow");
        let page = AreaFrame::new_mapped(self.family_registration.page_backend(logical_page), frame);
        let mut guard = page.ensure_page().expect("new anonymous file page is not resident");
        if dirty {
            guard.mark_dirty();
        }
        let kpage = guard.frame().get_page();
        addrspace.mmap_swappable(uaddr, &guard, self.perm);
        drop(guard);
        self.frames.insert(page_index, FrameState::Allocated(page));
        kpage
    }

    fn copy_on_write_page(
        &mut self,
        page_index: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<()> {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(
            self.perm.contains(MapPerm::W),
            "Original mapping must have write permission for copy-on-write"
        );

        let area_offset = page_index.checked_mul(arch::PGSIZE)?;
        let uaddr = self.ubase.checked_add(area_offset)?;
        map_change_notifier.before_map_change(MapChange {
            uaddr,
            page_count: 1,
            event: MapChangeEvent::Remap,
        });

        let old_page = match self.frames.get(page_index)? {
            FrameState::Cow(frame) => frame.clone(),
            _ => panic!("Invalid type for copy-on-write"),
        };
        if old_page.mapping_refs() == 1 {
            // The page is only mapped in this address space, we can just remap it as writable.
            let mut guard = old_page.ensure_page().ok()?;
            addrspace.mmap_replace_swappable_if_maps(uaddr, &guard, &guard, self.perm)?;
            guard.mark_dirty();
            drop(guard);
            self.frames.insert(page_index, FrameState::Allocated(old_page));
            return Some(());
        }

        // Allocate before taking the old page lock: allocation may invoke
        // reclaim, which must never select this page and wait on a lock
        // held by the allocating task itself.
        let new_frame = PhysPageFrame::alloc_with_shrink_zeroed();
        let mut old_guard = old_page.ensure_page().ok()?;

        new_frame.slice().copy_from_slice(old_guard.frame().slice());
        let logical_page = self.page_base.checked_add(page_index)?;
        let new_page = AreaFrame::new_mapped(self.family_registration.page_backend(logical_page), new_frame);
        let mut new_guard = new_page.ensure_page().ok()?;
        new_guard.mark_dirty();

        let Some(AccessDirty { dirty: old_dirty, .. }) =
            addrspace.mmap_replace_swappable_if_maps(uaddr, &old_guard, &new_guard, self.perm)
        else {
            new_guard.release_mapping_ref();
            return None;
        };
        if old_dirty {
            old_guard.mark_dirty();
        }

        self.frames.insert(page_index, FrameState::Allocated(new_page.clone()));
        old_guard.release_mapping_ref();
        Some(())
    }

    fn map_cow_page(&self, page_index: usize, frame: &Arc<AreaFrame>, addrspace: &AddrSpace) -> Option<usize> {
        let uaddr = self.ubase.checked_add(page_index.checked_mul(arch::PGSIZE)?)?;

        let guard = frame.ensure_page().ok()?;
        let kpage = guard.frame().get_page();
        addrspace.mmap_replace_swappable(uaddr, &guard, self.perm - MapPerm::W);
        Some(kpage)
    }
}

impl Area for PrivateFileMapArea {
    fn bind_addrspace(&mut self, addrspace: &Arc<AddrSpace>) {
        self.family_registration
            .bind(addrspace, self.ubase, self.page_base, self.frames.len());
    }

    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<PinPageFrame> {
        if uaddr < self.ubase {
            return None;
        }

        let area_offset = uaddr - self.ubase;
        let page_index = area_offset / arch::PGSIZE;
        if page_index >= self.frames.len() {
            return None;
        }

        if self.frames.get(page_index).is_none() {
            self.load_page(page_index, addrspace, false);
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
        if uaddr < self.ubase || !self.perm.contains(MapPerm::W) {
            return None;
        }

        let area_offset = uaddr - self.ubase;
        let page_index = area_offset / arch::PGSIZE;
        if page_index >= self.frames.len() {
            return None;
        }

        if self.frames.get(page_index).is_none() {
            self.load_page(page_index, addrspace, true);
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
        tlb_changed: &mut bool,
        addrspace: &Arc<AddrSpace>,
    ) -> Box<dyn Area> {
        let cow_perm = self.perm - MapPerm::W;

        let mut frames = AreaPageSlots::new(self.frames.len());
        for (page_index, frame) in self.frames.iter() {
            let page = match frame {
                FrameState::Allocated(page) | FrameState::Cow(page) => page,
            };
            page.add_mapping_ref();
            frames.insert(page_index, FrameState::Cow(page.clone()));
        }

        for (page_index, frame) in self.frames.iter_mut() {
            let FrameState::Allocated(page) = frame else {
                continue;
            };
            let page = page.clone();
            let uaddr = self
                .ubase
                .checked_add(
                    page_index
                        .checked_mul(arch::PGSIZE)
                        .expect("private file fork offset overflow"),
                )
                .expect("private file fork address overflow");
            page.with_resident_and_record_ad(false, |resident| {
                let access_dirty = self_pagetable.lock().mmap_replace_perm_with_check_and_ad_no_flush(
                    uaddr,
                    resident.get_page(),
                    cow_perm,
                );
                *tlb_changed |= access_dirty.is_some();
                let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                ((), AccessDirty { accessed, dirty })
            });
            *frame = FrameState::Cow(page);
        }

        let new_area = PrivateFileMapArea {
            ubase: self.ubase,
            perm: self.perm,
            file: self.file.clone(),
            file_offset: self.file_offset,
            file_length: self.file_length,
            family_registration: self.family_registration.fork(
                addrspace,
                self.ubase,
                self.page_base,
                self.frames.len(),
            ),
            page_base: self.page_base,
            frames,
        };

        Box::new(new_area)
    }

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        access_type: MemAccessType,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Result<(), MemoryFaultSignal> {
        if uaddr < self.ubase {
            return Err(MemoryFaultSignal::Segv);
        }

        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index >= self.frames.len() {
            return Err(MemoryFaultSignal::Segv);
        }

        if self.frames.get(page_index).is_none() {
            self.load_page(page_index, addrspace, access_type == MemAccessType::Write);
        } else if self.frames.get(page_index).is_some_and(FrameState::is_cow) {
            if access_type == MemAccessType::Write {
                self.copy_on_write_page(page_index, addrspace, map_change_notifier)
                    .ok_or(MemoryFaultSignal::Bus)?;
            } else {
                let Some(FrameState::Cow(frame)) = self.frames.get(page_index) else {
                    unreachable!();
                };
                self.map_cow_page(page_index, frame, addrspace)
                    .ok_or(MemoryFaultSignal::Bus)?;
            }
        } else {
            let Some(FrameState::Allocated(frame)) = self.frames.get(page_index) else {
                unreachable!();
            };
            let mut guard = frame.ensure_page().map_err(|_| MemoryFaultSignal::Bus)?;
            if access_type == MemAccessType::Write {
                guard.mark_dirty();
            }
            let page_uaddr = self
                .ubase
                .checked_add(page_index.checked_mul(arch::PGSIZE).ok_or(MemoryFaultSignal::Segv)?)
                .ok_or(MemoryFaultSignal::Segv)?;
            addrspace.mmap_replace_swappable(page_uaddr, &guard, self.perm);
        }

        if access_type == MemAccessType::Write {
            self.translate_write(uaddr, addrspace, map_change_notifier)
                .map(|_| ())
                .ok_or(MemoryFaultSignal::Segv)
        } else {
            self.translate_read(uaddr, addrspace)
                .map(|_| ())
                .ok_or(MemoryFaultSignal::Segv)
        }
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_ubase(&mut self, ubase: usize) {
        if self.family_registration.is_bound() {
            self.family_registration
                .update(ubase, self.page_base, self.frames.len());
        }
        self.ubase = ubase;
    }

    fn page_count(&self) -> usize {
        self.frames.len()
    }

    fn split(mut self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "Split address must be page-aligned");
        debug_assert!(uaddr > self.ubase, "Split address must be greater than ubase");
        debug_assert!(uaddr < self.ubase + self.size(), "Split address out of bounds");

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let split_offset = split_index
            .checked_mul(arch::PGSIZE)
            .expect("private file split offset overflow");
        let remaining_frames = self.frames.split_off(split_index);

        let new_file_length = self.file_length.saturating_sub(split_offset);
        self.file_length = core::cmp::min(self.file_length, split_offset);

        let right_page_base = self
            .page_base
            .checked_add(split_index)
            .expect("private file split anonymous page index overflow");
        let right_family_registration = if self.family_registration.is_bound() {
            self.family_registration.split(
                self.ubase,
                self.page_base,
                self.frames.len(),
                uaddr,
                right_page_base,
                remaining_frames.len(),
            )
        } else {
            self.family_registration.unbound_sibling()
        };

        let new_area = PrivateFileMapArea {
            ubase: uaddr,
            perm: self.perm,
            file: self.file.clone(),
            file_offset: self
                .file_offset
                .checked_add(split_offset)
                .expect("private file split file offset overflow"),
            file_length: new_file_length,
            family_registration: right_family_registration,
            page_base: right_page_base,
            frames: remaining_frames,
        };

        (self, Box::new(new_area))
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>, tlb_changed: &mut bool) {
        self.perm = perm;

        for (page_index, frame) in self.frames.iter() {
            let (frame, page_perm) = match frame {
                FrameState::Allocated(frame) => (frame, perm),
                FrameState::Cow(frame) => (frame, perm - MapPerm::W),
            };
            let uaddr = self
                .ubase
                .checked_add(
                    page_index
                        .checked_mul(arch::PGSIZE)
                        .expect("private file permission offset overflow"),
                )
                .expect("private file permission address overflow");
            frame.with_resident_and_record_ad(false, |resident| {
                let access_dirty = pagetable.lock().mmap_replace_perm_with_check_and_ad_no_flush(
                    uaddr,
                    resident.get_page(),
                    page_perm,
                );
                *tlb_changed |= access_dirty.is_some();
                let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                ((), AccessDirty { accessed, dirty })
            });
        }
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        self.family_registration.with_tlb_invalidation_batch(|token| {
            let mut tlb_changed = false;
            for (page_index, state) in self.frames.iter() {
                let page = match state {
                    FrameState::Allocated(page) | FrameState::Cow(page) => page,
                };
                let uaddr = self
                    .ubase
                    .checked_add(
                        page_index
                            .checked_mul(arch::PGSIZE)
                            .expect("private file unmap offset overflow"),
                    )
                    .expect("private file unmap address overflow");
                page.begin_tlb_invalidation(token, |resident| {
                    let access_dirty = pagetable
                        .lock()
                        .munmap_with_check_and_ad_no_flush(uaddr, resident.get_page());
                    tlb_changed |= access_dirty.is_some();
                    access_dirty.map(AccessDirty::from)
                });
            }
            if tlb_changed {
                arch::flush_tlb_all();
            }
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
        "PrivateFileMapArea"
    }

    fn map_area_info(&self) -> MapAreaInfo {
        let mut info = MapAreaInfo::new(self.ubase(), self.ubase() + self.size(), self.perm);
        info.offset = self.file_offset;
        (info.dev_major, info.dev_minor) = self.file.get_dentry().map(|dentry| (0, dentry.sno())).unwrap_or((0, 0));
        info.inode = self
            .file
            .get_dentry()
            .map(|dentry| dentry.ino() as u64)
            .or_else(|| self.file.get_inode().map(|inode| inode.get_ino() as u64))
            .unwrap_or(0);
        info.path = self.file.get_dentry().map(|dentry| dentry.get_path());
        info
    }
}

impl Drop for PrivateFileMapArea {
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
