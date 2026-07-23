use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::PageTable;
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::{
    Area, MapAreaInfo, MapChange, MapChangeEvent, MapChangeNotifier, MemoryFaultSignal, PinPageFrame,
};
use crate::kernel::mm::swappable::AccessDirty;
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType, PhysPageFrame};
use crate::klib::SpinLock;

use super::nofilemap::{AnonMapFamilyRegistration, FrameState, SwappablePageFrame};

pub enum AuxKey {
    _NULL = 0,
    _IGNORE = 1,
    _EXECFD = 2,
    PHDR = 3,
    PHENT = 4,
    PHNUM = 5,
    PAGESZ = 6,
    BASE = 7,
    FLAGS = 8,
    ENTRY = 9,
    _NOTELF = 10,
    _UID = 11,
    _EUID = 12,
    _GID = 13,
    _EGID = 14,
    RANDOM = 25,
}

const AUX_MAX: usize = 12;

pub struct Auxv {
    pub auxv: [usize; AUX_MAX * 2],
    pub length: usize,
}

impl Auxv {
    pub fn new() -> Self {
        Auxv {
            auxv: [0; AUX_MAX * 2],
            length: 0,
        }
    }

    pub fn push(&mut self, key: AuxKey, value: usize) {
        debug_assert!(
            self.length < AUX_MAX,
            "Auxiliary vector is full, cannot add more entries."
        );
        self.auxv[self.length * 2] = key as usize;
        self.auxv[self.length * 2 + 1] = value;
        self.length += 1;
    }
}

pub struct UserStack {
    ubase: usize,
    page_count: usize,
    frames: BTreeMap<usize, FrameState>,
    family_registration: AnonMapFamilyRegistration,
    page_base: usize,
}

impl UserStack {
    pub fn new() -> Self {
        Self {
            ubase: config::USER_STACK_TOP - config::USER_STACK_PAGE_COUNT_MAX * arch::PGSIZE,
            page_count: config::USER_STACK_PAGE_COUNT_MAX,
            frames: BTreeMap::new(),
            family_registration: AnonMapFamilyRegistration::new_root(),
            page_base: 0,
        }
    }

    fn get_max_page_count(&self) -> usize {
        config::USER_STACK_PAGE_COUNT_MAX
    }

    fn contains_uaddr(&self, uaddr: usize) -> bool {
        let Some(size) = self.page_count.checked_mul(arch::PGSIZE) else {
            return false;
        };
        self.ubase <= uaddr && uaddr < self.ubase.saturating_add(size)
    }

    fn anonymous_page_index(&self, stack_page_index: usize) -> usize {
        let uaddr = config::USER_STACK_TOP - (stack_page_index + 1) * arch::PGSIZE;
        self.page_base + (uaddr - self.ubase) / arch::PGSIZE
    }

    fn allocate_page(&mut self, page_index: usize, addrspace: &AddrSpace) -> usize {
        debug_assert!(
            page_index < self.get_max_page_count(),
            "Page index out of bounds: {}",
            page_index
        );
        debug_assert!(
            !self.frames.contains_key(&page_index),
            "Page at index {} is already allocated",
            page_index
        );

        let uaddr = config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE;
        debug_assert!(self.contains_uaddr(uaddr));
        let frame = PhysPageFrame::alloc_with_shrink_zeroed();
        let page = SwappablePageFrame::new_mapped(
            self.family_registration
                .page_backend(self.anonymous_page_index(page_index)),
            frame,
        );
        self.frames.insert(page_index, FrameState::Allocated(page.clone()));
        let guard = page.ensure_page().expect("new stack page must be resident");
        let kpage = guard.frame().get_page();
        addrspace.mmap_swappable(uaddr, &guard, MapPerm::R | MapPerm::W | MapPerm::U);
        kpage
    }

    fn copy_on_write_page(
        &mut self,
        page_index: usize,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Option<()> {
        debug_assert!(
            page_index < self.get_max_page_count(),
            "Page index out of bounds: {}",
            page_index
        );
        debug_assert!(
            matches!(self.frames.get(&page_index), Some(FrameState::Cow(_))),
            "Page at index {} is not allocated",
            page_index
        );

        let uaddr = config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE;
        map_change_notifier.before_map_change(MapChange {
            uaddr,
            page_count: 1,
            event: MapChangeEvent::Remap,
        });

        let old_page = match self.frames.get(&page_index) {
            Some(FrameState::Cow(page)) => page.clone(),
            _ => unreachable!(),
        };

        if old_page.mapping_refs() == 1 {
            // The page is only mapped in this address space, we can just remap it as writable.
            let mut guard = old_page.ensure_page().ok()?;
            addrspace.mmap_replace_swappable_if_maps(uaddr, &guard, &guard, MapPerm::R | MapPerm::W | MapPerm::U)?;
            guard.mark_dirty();
            drop(guard);
            self.frames.insert(page_index, FrameState::Allocated(old_page));
            return Some(());
        }

        let new_frame = PhysPageFrame::alloc_with_shrink_zeroed();
        let mut old_guard = old_page.ensure_page().ok()?;
        new_frame.slice().copy_from_slice(old_guard.frame().slice());
        let new_page = SwappablePageFrame::new_mapped(
            self.family_registration
                .page_backend(self.anonymous_page_index(page_index)),
            new_frame,
        );
        let Ok(mut new_guard) = new_page.ensure_page() else {
            new_page.release_mapping_ref();
            return None;
        };
        new_guard.mark_dirty();
        let Some(AccessDirty { dirty, .. }) = addrspace.mmap_replace_swappable_if_maps(
            uaddr,
            &old_guard,
            &new_guard,
            MapPerm::R | MapPerm::W | MapPerm::U,
        ) else {
            new_guard.release_mapping_ref();
            return None;
        };
        if dirty {
            old_guard.mark_dirty();
        }
        self.frames.insert(page_index, FrameState::Allocated(new_page.clone()));
        old_guard.release_mapping_ref();
        Some(())
    }

    fn map_cow_page(&self, page_index: usize, frame: &Arc<SwappablePageFrame>, addrspace: &AddrSpace) -> Option<usize> {
        let uaddr = config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE;

        let guard = frame.ensure_page().ok()?;
        let kpage = guard.frame().get_page();
        addrspace.mmap_replace_swappable(uaddr, &guard, MapPerm::R | MapPerm::U);
        Some(kpage)
    }

    fn handle_memory_fault_on_swapped_allocated(
        &self,
        page_index: usize,
        frame: &Arc<SwappablePageFrame>,
        addrspace: &AddrSpace,
    ) -> Option<usize> {
        let guard = frame.ensure_page().ok()?;
        let uaddr = config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE;
        let kpage = guard.frame().get_page();
        addrspace.mmap_replace_swappable(uaddr, &guard, MapPerm::R | MapPerm::W | MapPerm::U);
        Some(kpage)
    }

    fn push_buffer(&mut self, top: &mut usize, buffer: &[u8], addrspace: &AddrSpace) -> SysResult<()> {
        let total_len = buffer.len();
        let new_top = (*top).checked_sub(total_len).ok_or(Errno::E2BIG)?;
        if new_top < self.ubase() {
            return Err(Errno::E2BIG);
        }
        let mut uaddr = new_top;

        let mut copied = 0usize;
        let mut remaining = buffer.len();
        while remaining != 0 {
            let page_offset = uaddr & arch::PGMASK;
            let to_copy = core::cmp::min(arch::PGSIZE - page_offset, remaining);

            let page_index = (config::USER_STACK_TOP - uaddr - 1) / arch::PGSIZE;

            if !self.frames.contains_key(&page_index) {
                self.allocate_page(page_index, addrspace);
            }

            match self.frames.get(&page_index).unwrap() {
                FrameState::Allocated(frame) => {
                    let mut guard = frame.ensure_page().map_err(|_| Errno::EFAULT)?;
                    guard.mark_dirty();
                    let dst = &mut guard.frame().slice()[page_offset..page_offset + to_copy];
                    dst.copy_from_slice(&buffer[copied..copied + to_copy]);
                }
                _ => panic!("Page at index {} is not allocated", page_index),
            }

            uaddr += to_copy;
            copied += to_copy;
            remaining -= to_copy;
        }
        *top = new_top;

        Ok(())
    }

    fn push_c_str(&mut self, top: &mut usize, s: &str, addrspace: &AddrSpace) -> SysResult<()> {
        self.push_usize(top, 0, addrspace)?;
        self.push_buffer(top, s.as_bytes(), addrspace)
    }

    fn push_usize(&mut self, top: &mut usize, value: usize, addrspace: &AddrSpace) -> SysResult<()> {
        // *top &= !(core::mem::size_of::<usize>() - 1);
        self.push_buffer(top, &value.to_le_bytes(), addrspace)
    }

    fn push_auxv(&mut self, top: &mut usize, auxv: &Auxv, addrspace: &AddrSpace) -> SysResult<()> {
        self.push_usize(top, 0, addrspace)?;

        if auxv.length == 0 {
            return Ok(());
        }

        // SAFETY: `auxv.auxv` is initialized through `Auxv::new/push`; the
        // byte view is bounded by the initialized logical entry count and is
        // consumed immediately while constructing the user stack.
        let buffer = unsafe {
            core::slice::from_raw_parts(
                auxv.auxv.as_ptr() as *const u8,
                auxv.length * 2 * core::mem::size_of::<usize>(),
            )
        };
        self.push_buffer(top, &buffer, addrspace)?;

        Ok(())
    }

    /*
      HIGH
     +------------------+ <- config::USER_STACK_TOP
     | strings          |
     +------------------+
     | envp[n] = NULL   |
     | envp[n-1]        |
     | ...              |
     | envp[0]          |
     +------------------+
     | argv[argc] = NULL|
     | argv[argc-1]     |
     | ...              |
     | argv[0]          |
     +------------------+ <- user_stack_top + sizeof(usize)
     | argc             |
     +------------------+ <- user_stack_top
      LOW
    */
    /// Push arguments and environment variables onto the user stack.
    pub fn push_argv_envp_auxv(
        &mut self,
        argv: &[&str],
        envp: &[&str],
        auxv: &Auxv,
        addrspace: &AddrSpace,
    ) -> SysResult<usize> {
        let mut top = config::USER_STACK_TOP;

        let mut envp_ptrs = Vec::with_capacity(envp.len());
        for &env in envp.iter() {
            self.push_c_str(&mut top, env, addrspace)?;
            envp_ptrs.push(top);
        }

        let mut argv_ptrs = Vec::with_capacity(argv.len());
        for &arg in argv.iter() {
            self.push_c_str(&mut top, arg, addrspace)?;
            argv_ptrs.push(top);
        }

        // Padding for alignment
        let mut count_to_push = 0;
        count_to_push += 1; // auxv NULL
        count_to_push += auxv.length * 2; // auxv entries
        count_to_push += 1; // envp NULL
        count_to_push += envp.len(); // envp pointers
        count_to_push += 1; // argv NULL
        count_to_push += argv.len(); // argv pointers
        count_to_push += 1; // argc
        top &= !(16 - 1);
        top -= (16 - (count_to_push * core::mem::size_of::<usize>()) % 16) % 16;

        // Push auxiliary vector
        self.push_auxv(&mut top, auxv, addrspace)?;

        // Push envp pointers
        self.push_usize(&mut top, 0, addrspace)?;
        for &addr in envp_ptrs.iter().rev() {
            self.push_usize(&mut top, addr, addrspace)?;
        }

        // Push argv pointers
        self.push_usize(&mut top, 0, addrspace)?;
        for &addr in argv_ptrs.iter().rev() {
            self.push_usize(&mut top, addr, addrspace)?;
        }

        // Push argc
        self.push_usize(&mut top, argv.len(), addrspace)?;

        debug_assert!(top % 16 == 0, "User stack top is not aligned to 16 bytes: {:#x}", top);

        Ok(top)
    }
}

impl Drop for UserStack {
    fn drop(&mut self) {
        let frames = core::mem::take(&mut self.frames);
        for (_, state) in frames {
            let page = match state {
                FrameState::Allocated(page) | FrameState::Cow(page) => page,
            };
            page.release_mapping_ref();
        }
    }
}

impl Area for UserStack {
    fn bind_addrspace(&mut self, addrspace: &Arc<AddrSpace>) {
        self.family_registration
            .bind(addrspace, self.ubase, self.page_base, self.page_count);
    }

    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<PinPageFrame> {
        if !self.contains_uaddr(uaddr) {
            return None;
        }

        let page_index = (config::USER_STACK_TOP - uaddr - 1) / arch::PGSIZE;
        if page_index >= self.get_max_page_count() {
            return None;
        }
        if !self.frames.contains_key(&page_index) {
            self.allocate_page(page_index, addrspace);
        }
        let page = match self.frames.get(&page_index)? {
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
        if !self.contains_uaddr(uaddr) {
            return None;
        }

        let page_index = (config::USER_STACK_TOP - uaddr - 1) / arch::PGSIZE;
        if page_index >= self.get_max_page_count() {
            return None;
        }
        if !self.frames.contains_key(&page_index) {
            self.allocate_page(page_index, addrspace);
        }
        if matches!(self.frames.get(&page_index), Some(FrameState::Cow(_))) {
            self.copy_on_write_page(page_index, addrspace, map_change_notifier)?;
        }
        let page = match self.frames.get(&page_index)? {
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
        MapPerm::R | MapPerm::W | MapPerm::U
    }

    fn fork(
        &mut self,
        self_pagetable: &SpinLock<PageTable>,
        tlb_changed: &mut bool,
        addrspace: &Arc<AddrSpace>,
    ) -> Box<dyn Area> {
        let mut new_frames = BTreeMap::new();
        for (&index, state) in &self.frames {
            let page = match state {
                FrameState::Allocated(page) | FrameState::Cow(page) => page,
            };
            page.add_mapping_ref();
            new_frames.insert(index, FrameState::Cow(page.clone()));
        }

        for (&index, state) in &mut self.frames {
            let page = match state {
                FrameState::Allocated(page) | FrameState::Cow(page) => page.clone(),
            };
            let uaddr = config::USER_STACK_TOP - (index + 1) * arch::PGSIZE;
            page.with_resident_and_record_ad(false, |frame| {
                let access_dirty = self_pagetable.lock().mmap_replace_perm_with_check_and_ad_no_flush(
                    uaddr,
                    frame.get_page(),
                    MapPerm::R | MapPerm::U,
                );
                *tlb_changed |= access_dirty.is_some();
                let (accessed, dirty) = access_dirty.unwrap_or((false, false));
                ((), AccessDirty { accessed, dirty })
            });
            *state = FrameState::Cow(page);
        }

        Box::new(UserStack {
            ubase: self.ubase,
            page_count: self.page_count,
            frames: new_frames,
            family_registration: self
                .family_registration
                .fork(addrspace, self.ubase, self.page_base, self.page_count),
            page_base: self.page_base,
        })
    }

    fn try_to_fix_memory_fault(
        &mut self,
        addr: usize,
        access_type: MemAccessType,
        addrspace: &AddrSpace,
        map_change_notifier: &MapChangeNotifier<'_>,
    ) -> Result<(), MemoryFaultSignal> {
        if !self.contains_uaddr(addr) {
            return Err(MemoryFaultSignal::Segv);
        }

        let page_index = (config::USER_STACK_TOP - addr - 1) / arch::PGSIZE;

        if page_index >= self.get_max_page_count() {
            return Err(MemoryFaultSignal::Segv);
        }

        if access_type == MemAccessType::Write && matches!(self.frames.get(&page_index), Some(FrameState::Cow(_))) {
            self.copy_on_write_page(page_index, addrspace, map_change_notifier)
                .ok_or(MemoryFaultSignal::Bus)?;
        } else {
            match self.frames.get(&page_index) {
                Some(FrameState::Allocated(frame)) => {
                    self.handle_memory_fault_on_swapped_allocated(page_index, frame, addrspace)
                        .ok_or(MemoryFaultSignal::Bus)?;
                }
                Some(FrameState::Cow(frame)) => {
                    self.map_cow_page(page_index, frame, addrspace)
                        .ok_or(MemoryFaultSignal::Bus)?;
                }
                None => {
                    self.allocate_page(page_index, addrspace);
                }
            }
        }

        if access_type == MemAccessType::Write {
            self.translate_write(addr, addrspace, map_change_notifier)
                .map(|_| ())
                .ok_or(MemoryFaultSignal::Segv)
        } else {
            self.translate_read(addr, addrspace)
                .map(|_| ())
                .ok_or(MemoryFaultSignal::Segv)
        }
    }

    fn ubase(&self) -> usize {
        self.ubase
    }

    fn page_count(&self) -> usize {
        self.page_count
    }

    fn split(mut self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(
            uaddr >= self.ubase && uaddr < self.ubase + self.size(),
            "uaddr out of range for stack split, urange: [{:#x}, {:#x}), uaddr: {:#x}",
            self.ubase,
            self.ubase + self.size(),
            uaddr
        );

        let left_page_count = (uaddr - self.ubase) / arch::PGSIZE;
        let right_page_count = self.page_count - left_page_count;
        let split_stack_index = (config::USER_STACK_TOP - uaddr) / arch::PGSIZE;

        // Stack frame keys count down from USER_STACK_TOP. `split_off`
        // therefore extracts the lower-address (left) half.
        let left_frames = self.frames.split_off(&split_stack_index);
        let right_frames = core::mem::replace(&mut self.frames, left_frames);

        let right_page_base = self
            .page_base
            .checked_add(left_page_count)
            .expect("stack split page index overflow");
        let right_family_registration = self.family_registration.split(
            self.ubase,
            self.page_base,
            left_page_count,
            uaddr,
            right_page_base,
            right_page_count,
        );

        self.page_count = left_page_count;
        let right = Self {
            ubase: uaddr,
            page_count: right_page_count,
            frames: right_frames,
            family_registration: right_family_registration,
            page_base: right_page_base,
        };

        (self, Box::new(right))
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let frames = core::mem::take(&mut self.frames);
        self.family_registration.with_tlb_invalidation_batch(|token| {
            let mut tlb_changed = false;
            for (&page_index, state) in &frames {
                let page = match state {
                    FrameState::Allocated(page) | FrameState::Cow(page) => page,
                };
                let uaddr = config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE;
                page.begin_tlb_invalidation(token, |frame| {
                    let access_dirty = pagetable
                        .lock()
                        .munmap_with_check_and_ad_no_flush(uaddr, frame.get_page());
                    tlb_changed |= access_dirty.is_some();
                    access_dirty.map(AccessDirty::from)
                });
            }

            if tlb_changed {
                pagetable.lock().flush_tlb();
            }

            for state in frames.values() {
                let page = match state {
                    FrameState::Allocated(page) | FrameState::Cow(page) => page,
                };
                assert!(page.finish_tlb_invalidation(token));
            }
        });

        for (_, state) in frames {
            let page = match state {
                FrameState::Allocated(page) | FrameState::Cow(page) => page,
            };
            page.release_mapping_ref();
        }
        self.family_registration.unregister();
    }

    fn type_name(&self) -> &'static str {
        "stack"
    }

    fn map_area_info(&self) -> MapAreaInfo {
        let mut info = MapAreaInfo::new(self.ubase(), self.ubase() + self.size(), self.perm());
        info.path = Some("[stack]".into());
        info
    }
}
