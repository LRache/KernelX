use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::fs::file::{FileOps, RandomAccessFile};
use crate::kernel::mm::maparea::nofilemap::{AnonMapFamilyRegistration, FrameState, SwappablePageFrame as AreaFrame};
use crate::kernel::mm::maparea::{
    Area, MapAreaInfo, MapChange, MapChangeEvent, MapChangeNotifier, MemoryFaultSignal, PinPageFrame,
};
use crate::kernel::mm::swappable::{AccessDirty, FileMapRegistration, SharedFilePage, TlbInvalidationToken};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType, PhysPageFrame};
use crate::klib::SpinLock;

use super::super::slots::AreaPageSlots;

enum PrivateFilePage {
    Source(SharedFilePage),
    Anonymous(FrameState),
}

pub struct PrivateFileMapArea {
    ubase: usize,
    perm: MapPerm,

    file: Arc<RandomAccessFile>,
    file_offset: usize,
    file_length: usize,

    file_map_registration: Option<FileMapRegistration>,
    family_registration: AnonMapFamilyRegistration,
    page_base: usize,

    frames: AreaPageSlots<PrivateFilePage>,
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
        let file_map_registration = file
            .get_inode()
            .and_then(|inode| inode.file_mapping())
            .map(|mapping| mapping.registration());

        Self {
            ubase,
            perm,
            file,
            file_offset,
            file_length,
            file_map_registration,
            family_registration: AnonMapFamilyRegistration::new_root(),
            page_base: 0,
            frames: AreaPageSlots::new(page_count),
        }
    }

    fn file_page_base(&self) -> usize {
        self.file_offset / arch::PGSIZE
    }

    fn file_page_index(&self, page_index: usize) -> Option<usize> {
        self.file_page_base().checked_add(page_index)
    }

    fn page_uaddr(&self, page_index: usize) -> Option<usize> {
        self.ubase.checked_add(page_index.checked_mul(arch::PGSIZE)?)
    }

    fn ensure_source_page(&mut self, page_index: usize) -> Option<&SharedFilePage> {
        if page_index >= self.frames.len() {
            return None;
        }

        let needs_load = match self.frames.get(page_index) {
            None => true,
            Some(PrivateFilePage::Source(SharedFilePage::Stable(_))) => false,
            Some(PrivateFilePage::Source(SharedFilePage::Swappable(page))) => page.is_invalid(),
            Some(PrivateFilePage::Anonymous(_)) => return None,
        };
        if needs_load {
            // Replacing an invalid source page drops its old mapping pin.
            self.frames.remove(page_index);
            let file_page_index = self.file_page_index(page_index)?;
            let page = self
                .file
                .get_inode()?
                .acquire_mmap_shared_page(file_page_index)
                .ok()??;
            self.frames.insert(page_index, PrivateFilePage::Source(page));
        }

        match self.frames.get(page_index) {
            Some(PrivateFilePage::Source(page)) => Some(page),
            _ => None,
        }
    }

    fn install_anonymous_copy(
        &mut self,
        page_index: usize,
        source_frame: &PhysPageFrame,
        new_frame: PhysPageFrame,
        addrspace: &AddrSpace,
    ) -> Option<Arc<AreaFrame>> {
        new_frame.slice().copy_from_slice(source_frame.slice());

        let logical_page = self.page_base.checked_add(page_index)?;
        let new_page = AreaFrame::new_mapped(self.family_registration.page_backend(logical_page), new_frame);
        let mut new_guard = new_page.ensure_page().ok()?;
        new_guard.mark_dirty();

        let uaddr = self.page_uaddr(page_index)?;
        if addrspace
            .mmap_replace_swappable_if_frame(uaddr, source_frame, &new_guard, self.perm)
            .is_none()
        {
            new_guard.release_mapping_ref();
            return None;
        }

        drop(new_guard);
        self.frames.insert(
            page_index,
            PrivateFilePage::Anonymous(FrameState::Allocated(new_page.clone())),
        );
        Some(new_page)
    }

    fn copy_source_page(
        &mut self,
        page_index: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<Arc<AreaFrame>> {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(self.perm.contains(MapPerm::W));

        let uaddr = self.page_uaddr(page_index)?;
        map_change_notifier.before_map_change(MapChange {
            uaddr,
            page_count: 1,
            event: MapChangeEvent::Remap,
        });

        let source = match self.frames.remove(page_index)? {
            PrivateFilePage::Source(source) => source,
            state => {
                self.frames.insert(page_index, state);
                panic!("Invalid type for private file copy-on-write");
            }
        };

        // Allocate before taking the source page lock: allocation may reclaim
        // a file page and must not wait on a lock held by this task.
        let new_frame = PhysPageFrame::alloc_with_shrink_zeroed();
        let copied = match &source {
            SharedFilePage::Stable(source_frame) => {
                self.install_anonymous_copy(page_index, source_frame, new_frame, addrspace)
            }
            SharedFilePage::Swappable(source_page) => match source_page.ensure_page() {
                Ok(source_guard) => self.install_anonymous_copy(page_index, source_guard.frame(), new_frame, addrspace),
                Err(_) => None,
            },
        };

        if copied.is_none() {
            self.frames.insert(page_index, PrivateFilePage::Source(source));
        }
        copied
    }

    fn copy_on_write_page(
        &mut self,
        page_index: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<Arc<AreaFrame>> {
        debug_assert!(page_index < self.frames.len());
        debug_assert!(
            self.perm.contains(MapPerm::W),
            "Original mapping must have write permission for copy-on-write"
        );

        let uaddr = self.page_uaddr(page_index)?;
        map_change_notifier.before_map_change(MapChange {
            uaddr,
            page_count: 1,
            event: MapChangeEvent::Remap,
        });

        let old_page = match self.frames.get(page_index)? {
            PrivateFilePage::Anonymous(FrameState::Cow(frame)) => frame.clone(),
            _ => panic!("Invalid type for copy-on-write"),
        };
        if old_page.mapping_refs() == 1 {
            // The page is only mapped in this address space, we can just remap it as writable.
            let mut guard = old_page.ensure_page().ok()?;
            addrspace.mmap_replace_swappable_if_maps(uaddr, &guard, &guard, self.perm)?;
            guard.mark_dirty();
            drop(guard);
            self.frames.insert(
                page_index,
                PrivateFilePage::Anonymous(FrameState::Allocated(old_page.clone())),
            );
            return Some(old_page);
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

        drop(new_guard);
        self.frames.insert(
            page_index,
            PrivateFilePage::Anonymous(FrameState::Allocated(new_page.clone())),
        );
        old_guard.release_mapping_ref();
        Some(new_page)
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
        let file_page_base = self.file_page_base();
        if let Some(registration) = &mut self.file_map_registration {
            registration.bind(addrspace, self.ubase, file_page_base, self.frames.len());
        }
        self.family_registration
            .bind(addrspace, self.ubase, self.page_base, self.frames.len());
    }

    fn translate_read(&mut self, uaddr: usize, _addrspace: &AddrSpace) -> Option<PinPageFrame> {
        if uaddr < self.ubase {
            return None;
        }

        let area_offset = uaddr - self.ubase;
        let page_index = area_offset / arch::PGSIZE;
        if page_index >= self.frames.len() {
            return None;
        }

        if !matches!(self.frames.get(page_index), Some(PrivateFilePage::Anonymous(_))) {
            self.ensure_source_page(page_index)?;
        }
        match self.frames.get(page_index)? {
            PrivateFilePage::Source(SharedFilePage::Stable(frame)) => Some(PinPageFrame::stable(frame.clone())),
            PrivateFilePage::Source(SharedFilePage::Swappable(page)) => {
                Some(PinPageFrame::file_swappable(page.pin_page(false).ok()?))
            }
            PrivateFilePage::Anonymous(FrameState::Allocated(page))
            | PrivateFilePage::Anonymous(FrameState::Cow(page)) => {
                Some(PinPageFrame::swappable(page.pin_page(false).ok()?))
            }
        }
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

        if !matches!(self.frames.get(page_index), Some(PrivateFilePage::Anonymous(_))) {
            self.ensure_source_page(page_index)?;
        }
        let page = match self.frames.get(page_index)? {
            PrivateFilePage::Source(_) => self.copy_source_page(page_index, addrspace, map_change_notifier)?,
            PrivateFilePage::Anonymous(FrameState::Cow(_)) => {
                self.copy_on_write_page(page_index, addrspace, map_change_notifier)?
            }
            PrivateFilePage::Anonymous(FrameState::Allocated(page)) => page.clone(),
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
        for (page_index, state) in self.frames.iter() {
            let state = match state {
                PrivateFilePage::Source(page) => PrivateFilePage::Source(page.clone()),
                PrivateFilePage::Anonymous(FrameState::Allocated(page))
                | PrivateFilePage::Anonymous(FrameState::Cow(page)) => {
                    page.add_mapping_ref();
                    PrivateFilePage::Anonymous(FrameState::Cow(page.clone()))
                }
            };
            frames.insert(page_index, state);
        }

        for (page_index, state) in self.frames.iter_mut() {
            let PrivateFilePage::Anonymous(FrameState::Allocated(page)) = state else {
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
                let access_dirty =
                    self_pagetable
                        .lock()
                        .mmap_replace_perm_with_check_and_ad(uaddr, resident.get_page(), cow_perm);
                *tlb_changed |= access_dirty.is_some();
                let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                ((), AccessDirty { accessed, dirty })
            });
            *state = PrivateFilePage::Anonymous(FrameState::Cow(page));
        }

        let new_area = PrivateFileMapArea {
            ubase: self.ubase,
            perm: self.perm,
            file: self.file.clone(),
            file_offset: self.file_offset,
            file_length: self.file_length,
            file_map_registration: self
                .file_map_registration
                .as_ref()
                .map(|registration| registration.fork(addrspace, self.ubase, self.file_page_base(), self.frames.len())),
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

        let remap_allocated_on_write = matches!(
            self.frames.get(page_index),
            Some(PrivateFilePage::Anonymous(FrameState::Allocated(_)))
        );
        if access_type == MemAccessType::Write {
            self.translate_write(uaddr, addrspace, map_change_notifier)
                .map(|_| ())
                .ok_or(MemoryFaultSignal::Bus)?;
            if remap_allocated_on_write {
                let Some(PrivateFilePage::Anonymous(FrameState::Allocated(frame))) = self.frames.get(page_index) else {
                    unreachable!();
                };
                let guard = frame.ensure_page().map_err(|_| MemoryFaultSignal::Bus)?;
                let page_uaddr = self.page_uaddr(page_index).ok_or(MemoryFaultSignal::Segv)?;
                addrspace.mmap_replace_swappable(page_uaddr, &guard, self.perm);
            }
            return Ok(());
        }

        if !matches!(self.frames.get(page_index), Some(PrivateFilePage::Anonymous(_))) {
            self.ensure_source_page(page_index).ok_or(MemoryFaultSignal::Bus)?;
        }

        let page_uaddr = self.page_uaddr(page_index).ok_or(MemoryFaultSignal::Segv)?;
        match self.frames.get(page_index).ok_or(MemoryFaultSignal::Bus)? {
            PrivateFilePage::Source(SharedFilePage::Stable(frame)) => {
                addrspace
                    .pagetable()
                    .lock()
                    .mmap_replace(page_uaddr, frame, self.perm - MapPerm::W);
                addrspace.pagetable().lock().flush_tlb();
            }
            PrivateFilePage::Source(SharedFilePage::Swappable(page)) => {
                let guard = page.ensure_page().map_err(|_| MemoryFaultSignal::Bus)?;
                addrspace.mmap_replace_swappable(page_uaddr, &guard, self.perm - MapPerm::W);
            }
            PrivateFilePage::Anonymous(FrameState::Cow(frame)) => {
                self.map_cow_page(page_index, frame, addrspace)
                    .ok_or(MemoryFaultSignal::Bus)?;
            }
            PrivateFilePage::Anonymous(FrameState::Allocated(frame)) => {
                let guard = frame.ensure_page().map_err(|_| MemoryFaultSignal::Bus)?;
                addrspace.mmap_replace_swappable(page_uaddr, &guard, self.perm);
            }
        }

        Ok(())
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_ubase(&mut self, ubase: usize) {
        if let Some(registration) = &self.file_map_registration
            && registration.is_bound()
        {
            registration.update(ubase, self.file_page_base(), self.frames.len());
        }
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

        let right_file_page_base = self
            .file_page_base()
            .checked_add(split_index)
            .expect("private file split file page index overflow");
        let left_file_page_base = self.file_page_base();
        let right_file_map_registration = self.file_map_registration.as_mut().map(|registration| {
            registration.split(
                self.ubase,
                left_file_page_base,
                self.frames.len(),
                uaddr,
                right_file_page_base,
                remaining_frames.len(),
            )
        });

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
            file_map_registration: right_file_map_registration,
            family_registration: right_family_registration,
            page_base: right_page_base,
            frames: remaining_frames,
        };

        (self, Box::new(new_area))
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>, tlb_changed: &mut bool) {
        self.perm = perm;

        for (page_index, state) in self.frames.iter() {
            let uaddr = self
                .ubase
                .checked_add(
                    page_index
                        .checked_mul(arch::PGSIZE)
                        .expect("private file permission offset overflow"),
                )
                .expect("private file permission address overflow");
            match state {
                PrivateFilePage::Source(SharedFilePage::Stable(frame)) => {
                    let access_dirty = pagetable.lock().mmap_replace_perm_with_check_and_ad(
                        uaddr,
                        frame.get_page(),
                        perm - MapPerm::W,
                    );
                    *tlb_changed |= access_dirty.is_some();
                }
                PrivateFilePage::Source(SharedFilePage::Swappable(frame)) => {
                    frame.with_resident_and_record_ad(false, |resident| {
                        let access_dirty = pagetable.lock().mmap_replace_perm_with_check_and_ad(
                            uaddr,
                            resident.get_page(),
                            perm - MapPerm::W,
                        );
                        *tlb_changed |= access_dirty.is_some();
                        let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                        ((), AccessDirty { accessed, dirty })
                    });
                }
                PrivateFilePage::Anonymous(state) => {
                    let (frame, page_perm) = match state {
                        FrameState::Allocated(frame) => (frame, perm),
                        FrameState::Cow(frame) => (frame, perm - MapPerm::W),
                    };
                    frame.with_resident_and_record_ad(false, |resident| {
                        let access_dirty =
                            pagetable
                                .lock()
                                .mmap_replace_perm_with_check_and_ad(uaddr, resident.get_page(), page_perm);
                        *tlb_changed |= access_dirty.is_some();
                        let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                        ((), AccessDirty { accessed, dirty })
                    });
                }
            }
        }
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let unmap_source_pages = |token: Option<&TlbInvalidationToken>| {
            let mut tlb_changed = false;
            for (page_index, state) in self.frames.iter() {
                let PrivateFilePage::Source(source) = state else {
                    continue;
                };
                let uaddr = self
                    .page_uaddr(page_index)
                    .expect("private file source unmap address overflow");
                match source {
                    SharedFilePage::Stable(frame) => {
                        tlb_changed |= pagetable.lock().munmap_with_check(uaddr, frame.get_page());
                    }
                    SharedFilePage::Swappable(page) => {
                        let token = token.expect("swappable source page must have a file mapping registration");
                        page.begin_tlb_invalidation(token, |resident| {
                            let access_dirty = pagetable.lock().munmap_with_check_and_ad(uaddr, resident.get_page());
                            tlb_changed |= access_dirty.is_some();
                            access_dirty.map(AccessDirty::from)
                        });
                    }
                }
            }
            if tlb_changed {
                pagetable.lock().flush_tlb();
            }
            for (_, state) in self.frames.iter() {
                if let PrivateFilePage::Source(SharedFilePage::Swappable(page)) = state {
                    let token = token.expect("swappable source page must have a file mapping registration");
                    assert!(page.finish_tlb_invalidation(token));
                }
            }
        };
        if let Some(registration) = &self.file_map_registration {
            registration.with_tlb_invalidation_batch(|token| unmap_source_pages(Some(token)));
        } else {
            unmap_source_pages(None);
        }

        self.family_registration.with_tlb_invalidation_batch(|token| {
            let mut tlb_changed = false;
            for (page_index, state) in self.frames.iter() {
                let page = match state {
                    PrivateFilePage::Anonymous(FrameState::Allocated(page))
                    | PrivateFilePage::Anonymous(FrameState::Cow(page)) => page,
                    PrivateFilePage::Source(_) => continue,
                };
                let uaddr = self
                    .page_uaddr(page_index)
                    .expect("private file unmap address overflow");
                page.begin_tlb_invalidation(token, |resident| {
                    let access_dirty = pagetable.lock().munmap_with_check_and_ad(uaddr, resident.get_page());
                    tlb_changed |= access_dirty.is_some();
                    access_dirty.map(AccessDirty::from)
                });
            }
            if tlb_changed {
                pagetable.lock().flush_tlb();
            }
            for (_, state) in self.frames.iter() {
                let page = match state {
                    PrivateFilePage::Anonymous(FrameState::Allocated(page))
                    | PrivateFilePage::Anonymous(FrameState::Cow(page)) => page,
                    PrivateFilePage::Source(_) => continue,
                };
                assert!(page.finish_tlb_invalidation(token));
            }
        });

        self.frames.clear_with(|_, state| {
            let page = match state {
                PrivateFilePage::Anonymous(FrameState::Allocated(page))
                | PrivateFilePage::Anonymous(FrameState::Cow(page)) => page,
                PrivateFilePage::Source(_) => return,
            };
            page.release_mapping_ref();
        });
        if let Some(registration) = &mut self.file_map_registration {
            registration.unregister();
        }
        self.family_registration.unregister();
    }

    fn type_name(&self) -> &'static str {
        "PrivateFileMapArea"
    }

    // PERF_DEBUG(map-manager-lock-backing): Remove with Area::debug_backing_id.
    #[cfg(feature = "map-manager-lock-debug")]
    fn debug_backing_id(&self) -> usize {
        Arc::as_ptr(&self.file) as usize
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
                PrivateFilePage::Anonymous(FrameState::Allocated(page))
                | PrivateFilePage::Anonymous(FrameState::Cow(page)) => page,
                PrivateFilePage::Source(_) => return,
            };
            page.release_mapping_ref();
        });
        if let Some(registration) = &mut self.file_map_registration {
            registration.unregister();
        }
        self.family_registration.unregister();
    }
}
