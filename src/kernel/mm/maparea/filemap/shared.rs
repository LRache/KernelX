use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::fs::Inode;
use crate::fs::inode::Index as InodeIndex;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::{Area, MapAreaInfo, MapChangeNotifier, MemoryFaultSignal, PinPageFrame};
use crate::kernel::mm::swappable::{AccessDirty, FileMapRegistration, SharedFilePage, TlbInvalidationToken};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::kernel::uapi::FileSealFlags;
use crate::klib::SpinLock;

enum FrameState {
    Unallocated,
    Loaded(SharedFilePage),
}

pub struct SharedFileMapArea {
    /* Inode basic */
    inode: Arc<Inode>,
    inode_index: InodeIndex,

    ubase: usize,
    offset: usize,
    states: Vec<FrameState>,
    perm: MapPerm,
    writable: bool,

    /// The path of the file for which this area is mapped.
    /// For procfs `map_area_info`
    path: String,

    /* For swappable pages */
    file_map_registration: Option<FileMapRegistration>,
}

impl SharedFileMapArea {
    pub fn new(
        ubase: usize,
        perm: MapPerm,
        inode: Arc<Inode>,
        index: InodeIndex,
        offset: usize,
        length: usize,
        writable: bool,
        path: String,
    ) -> Self {
        let page_count = arch::page_count(length);
        let is_writable = perm.contains(MapPerm::W);
        if let Some(seal_ops) = inode.as_seal_ops() {
            seal_ops.begin_shared_mmap(is_writable);
        }

        let file_map_registration = inode.file_mapping().map(|mapping| mapping.registration());
        let mut states = Vec::with_capacity(page_count);
        states.resize_with(page_count, || FrameState::Unallocated);

        Self {
            inode,
            file_map_registration,
            ubase,
            offset,
            states,
            perm,
            writable,
            inode_index: index,
            path,
        }
    }

    fn file_page_index(&self, page_index: usize) -> Option<usize> {
        (self.offset / arch::PGSIZE).checked_add(page_index)
    }

    fn file_page_base(&self) -> usize {
        self.offset / arch::PGSIZE
    }

    /// Ensures that this area owns the page's mapping reference. A swappable
    /// page is not kept resident after this function returns.
    /// Returns `Ok(None)` if the page index is out of bounds or the page cannot be loaded.
    fn ensure_page(&mut self, page_index: usize) -> SysResult<Option<&SharedFilePage>> {
        if page_index >= self.states.len() {
            return Ok(None);
        }

        let needs_load = match &self.states[page_index] {
            FrameState::Unallocated => true,
            FrameState::Loaded(SharedFilePage::Stable(_)) => false,
            FrameState::Loaded(SharedFilePage::Swappable(page)) => page.is_invalid(),
        };
        if needs_load {
            // Replacing an invalid swappable page drops its old mapping pin.
            self.states[page_index] = FrameState::Unallocated;
            let file_page_index = self.file_page_index(page_index).ok_or(Errno::EFBIG)?;
            let Some(page) = self.inode.acquire_mmap_shared_page(file_page_index)? else {
                return Ok(None);
            };
            self.states[page_index] = FrameState::Loaded(page);
        }

        let FrameState::Loaded(page) = &self.states[page_index] else {
            unreachable!();
        };
        Ok(Some(page))
    }

    fn translate(&mut self, uaddr: usize, write: bool) -> Option<PinPageFrame> {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page = self.ensure_page(page_index).ok()??;
        match page {
            SharedFilePage::Stable(frame) => Some(PinPageFrame::stable(frame.clone())),
            SharedFilePage::Swappable(page) => {
                let pin = page.pin_page(write).ok()?;
                Some(PinPageFrame::file_swappable(pin))
            }
        }
    }
}

impl Area for SharedFileMapArea {
    fn bind_addrspace(&mut self, addrspace: &Arc<AddrSpace>) {
        let ubase = self.ubase;
        let file_page_base = self.file_page_base();
        let page_count = self.states.len();
        if let Some(registration) = &mut self.file_map_registration
            && !registration.is_bound()
        {
            registration.bind(addrspace, ubase, file_page_base, page_count);
        }
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_ubase(&mut self, ubase: usize) {
        self.ubase = ubase;
        if let Some(registration) = &self.file_map_registration
            && registration.is_bound()
        {
            registration.update(self.ubase, self.file_page_base(), self.states.len());
        }
    }

    fn perm(&self) -> MapPerm {
        self.perm
    }

    fn check_set_perm(&self, perm: MapPerm) -> SysResult<()> {
        if perm.contains(MapPerm::W) {
            if !self.writable {
                return Err(Errno::EACCES);
            }

            if let Some(seal_ops) = self.inode.as_seal_ops() {
                if let Ok(seals) = seal_ops.seals() {
                    if seals.contains(FileSealFlags::F_SEAL_WRITE)
                        || (!self.perm.contains(MapPerm::W) && seals.contains(FileSealFlags::F_SEAL_FUTURE_WRITE))
                    {
                        return Err(Errno::EPERM);
                    }
                }
            }
        }

        Ok(())
    }

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>, tlb_changed: &mut bool) {
        let old_writable = self.perm.contains(MapPerm::W);
        let new_writable = perm.contains(MapPerm::W);
        if old_writable != new_writable {
            if let Some(seal_ops) = self.inode.as_seal_ops() {
                seal_ops.update_shared_mmap_writable(old_writable, new_writable);
            }
        }
        self.perm = perm;

        for (page_index, state) in self.states.iter().enumerate() {
            let FrameState::Loaded(page) = state else {
                continue;
            };
            let uaddr = self.ubase + page_index * arch::PGSIZE;
            match page {
                SharedFilePage::Stable(frame) => {
                    *tlb_changed |= pagetable.lock().mmap_replace_perm_no_flush(uaddr, perm);
                    let _ = frame;
                }
                SharedFilePage::Swappable(page) => {
                    page.with_resident_and_record_ad(false, |frame| {
                        let access_dirty = pagetable.lock().mmap_replace_perm_with_check_and_ad_no_flush(
                            uaddr,
                            frame.get_page(),
                            perm,
                        );
                        *tlb_changed |= access_dirty.is_some();
                        let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                        ((), AccessDirty { accessed, dirty })
                    });
                }
            }
        }
    }

    fn page_count(&self) -> usize {
        self.states.len()
    }

    fn size(&self) -> usize {
        self.states.len() * arch::PGSIZE
    }

    fn fork(
        &mut self,
        _self_pagetable: &SpinLock<PageTable>,
        _tlb_changed: &mut bool,
        addrspace: &Arc<AddrSpace>,
    ) -> Box<dyn Area> {
        let is_writable = self.perm.contains(MapPerm::W);
        if let Some(seal_ops) = self.inode.as_seal_ops() {
            seal_ops.begin_shared_mmap(is_writable);
        }
        let mut states = Vec::with_capacity(self.states.len());
        states.resize_with(self.states.len(), || FrameState::Unallocated);
        let new_area = SharedFileMapArea {
            inode: self.inode.clone(),
            file_map_registration: self
                .file_map_registration
                .as_ref()
                .map(|registration| registration.fork(addrspace, self.ubase, self.file_page_base(), self.states.len())),
            ubase: self.ubase,
            offset: self.offset,
            states,
            perm: self.perm,
            writable: self.writable,
            inode_index: self.inode_index,
            path: self.path.clone(),
        };

        Box::new(new_area)
    }

    fn translate_read(&mut self, uaddr: usize, _addrspace: &AddrSpace) -> Option<PinPageFrame> {
        self.translate(uaddr, false)
    }

    fn translate_write(
        &mut self,
        uaddr: usize,
        _addrspace: &AddrSpace,
        _map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<PinPageFrame> {
        self.translate(uaddr, true)
    }

    fn get_frame(
        &mut self,
        uaddr: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<PinPageFrame> {
        self.translate_write(uaddr, addrspace, map_change_notifier)
    }

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        _access_type: MemAccessType,
        addrspace: &AddrSpace,
        _map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Result<(), MemoryFaultSignal> {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index >= self.states.len() {
            return Err(MemoryFaultSignal::Segv);
        }

        let uaddr = self.ubase + page_index * arch::PGSIZE;
        let perm = self.perm;
        let page = self
            .ensure_page(page_index)
            .map_err(|_| MemoryFaultSignal::Bus)?
            .ok_or(MemoryFaultSignal::Bus)?;
        match page {
            SharedFilePage::Stable(frame) => addrspace.pagetable().lock().mmap_replace(uaddr, frame, perm),
            SharedFilePage::Swappable(page) => {
                let guard = page.ensure_page().map_err(|_| MemoryFaultSignal::Bus)?;
                addrspace.mmap_replace_swappable(uaddr, &guard, perm);
            }
        }

        Ok(())
    }

    fn split(mut self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "Split address must be page-aligned");
        debug_assert!(uaddr > self.ubase, "Split address must be greater than ubase");
        debug_assert!(uaddr < self.ubase + self.size(), "Split address out of bounds");

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let split_offset = split_index * arch::PGSIZE;
        let right_states = self.states.split_off(split_index);
        let right_file_page_base = self
            .file_page_base()
            .checked_add(split_index)
            .expect("shared file split page index overflow");
        let left_file_page_base = self.file_page_base();
        let left_page_count = self.states.len();
        let right_file_map_registration = self.file_map_registration.as_mut().map(|registration| {
            registration.split(
                self.ubase,
                left_file_page_base,
                left_page_count,
                uaddr,
                right_file_page_base,
                right_states.len(),
            )
        });

        let is_writable = self.perm.contains(MapPerm::W);
        if let Some(seal_ops) = self.inode.as_seal_ops() {
            seal_ops.begin_shared_mmap(is_writable);
        }
        let right = SharedFileMapArea {
            inode: self.inode.clone(),
            file_map_registration: right_file_map_registration,
            ubase: uaddr,
            offset: self.offset + split_offset,
            states: right_states,
            perm: self.perm,
            writable: self.writable,
            inode_index: self.inode_index,
            path: self.path.clone(),
        };

        (self, Box::new(right))
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let invalidate = |token: Option<&TlbInvalidationToken>| {
            let mut tlb_changed = false;
            for (page_index, state) in self.states.iter().enumerate() {
                let FrameState::Loaded(page) = state else {
                    continue;
                };
                let uaddr = self.ubase + page_index * arch::PGSIZE;
                match page {
                    SharedFilePage::Stable(frame) => {
                        tlb_changed |= pagetable.lock().munmap_with_check_no_flush(uaddr, frame.get_page());
                    }
                    SharedFilePage::Swappable(page) => {
                        let token = token.expect("swappable file page must have a file mapping registration");
                        page.begin_tlb_invalidation(token, |frame| {
                            let access_dirty = pagetable
                                .lock()
                                .munmap_with_check_and_ad_no_flush(uaddr, frame.get_page());
                            tlb_changed |= access_dirty.is_some();
                            access_dirty.map(AccessDirty::from)
                        });
                    }
                }
            }
            if tlb_changed {
                arch::flush_tlb_all();
            }
            for state in &self.states {
                if let FrameState::Loaded(SharedFilePage::Swappable(page)) = state {
                    let token = token.expect("swappable file page must have a file mapping registration");
                    assert!(page.finish_tlb_invalidation(token));
                }
            }
        };
        if let Some(registration) = &self.file_map_registration {
            registration.with_tlb_invalidation_batch(|token| invalidate(Some(token)));
        } else {
            invalidate(None);
        }

        for state in &mut self.states {
            *state = FrameState::Unallocated;
        }

        if let Some(registration) = &mut self.file_map_registration {
            registration.unregister();
        }
    }

    fn type_name(&self) -> &'static str {
        "SharedFileMapArea"
    }

    fn map_area_info(&self) -> MapAreaInfo {
        let mut info = MapAreaInfo::new(self.ubase(), self.ubase() + self.size(), self.perm);
        info.shared = true;
        info.offset = self.offset;
        info.dev_minor = self.inode_index.sno; // TODO: Use device number instead of inode number for dev_minor.
        info.inode = self.inode_index.ino as u64;
        info.path = Some(self.path.clone());
        info
    }
}

impl Drop for SharedFileMapArea {
    fn drop(&mut self) {
        for state in &mut self.states {
            *state = FrameState::Unallocated;
        }
        if let Some(registration) = &mut self.file_map_registration {
            registration.unregister();
        }
        if let Some(seal_ops) = self.inode.as_seal_ops() {
            seal_ops.end_shared_mmap(self.perm.contains(MapPerm::W));
        }
    }
}
