use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::kernel::config;
use crate::kernel::errno::SysResult;
use crate::kernel::mm::maparea::area::{Area, MapAreaInfo, MemoryFaultSignal};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::klib::SpinLock;

use super::nofilemap::FrameState;

#[cfg(feature = "swap-memory")]
use super::nofilemap::SwappableNoFileFrame;

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
    frames: Vec<FrameState>,
}

impl UserStack {
    pub fn new() -> Self {
        let frames = Vec::from_iter((0..config::USER_STACK_PAGE_COUNT_MAX).map(|_| FrameState::Unallocated));
        Self { frames }
    }

    fn get_max_page_count(&self) -> usize {
        self.frames.len()
    }

    fn allocate_page(&mut self, page_index: usize, pagetable: &mut PageTable, addrspace: &AddrSpace) -> usize {
        debug_assert!(
            page_index < self.get_max_page_count(),
            "Page index out of bounds: {}",
            page_index
        );
        debug_assert!(
            self.frames[page_index].is_unallocated(),
            "Page at index {} is already allocated",
            page_index
        );

        let uaddr = config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE;
        let (allocated, kpage) = FrameState::allocate(uaddr, addrspace);

        pagetable.mmap(uaddr, kpage, MapPerm::R | MapPerm::W | MapPerm::U);

        self.frames[page_index] = allocated;

        kpage
    }

    fn copy_on_write_page(&mut self, page_index: usize, addrspace: &AddrSpace) -> usize {
        debug_assert!(
            page_index < self.get_max_page_count(),
            "Page index out of bounds: {}",
            page_index
        );
        debug_assert!(
            self.frames[page_index].is_cow(),
            "Page at index {} is not allocated",
            page_index
        );

        let kpage = self.frames[page_index].cow_to_allocated(addrspace);

        addrspace.pagetable().lock().mmap_replace(
            config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE,
            kpage,
            MapPerm::R | MapPerm::W | MapPerm::U,
        );

        kpage
    }

    #[cfg(feature = "swap-memory")]
    fn handle_memory_fault_on_swapped_allocated(&self, frame: &SwappableNoFileFrame, addrspace: &AddrSpace) {
        debug_assert!(frame.is_swapped_out(), "Frame is not swapped out");
        let kpage = frame.get_page_swap_in();
        addrspace
            .pagetable()
            .write()
            .mmap(frame.uaddr(), kpage, MapPerm::R | MapPerm::W | MapPerm::U);
    }

    #[cfg(feature = "swap-memory")]
    fn handle_cow_read_swapped_out(&self, frame: &SwappableNoFileFrame, addrspace: &AddrSpace) {
        debug_assert!(frame.is_swapped_out(), "Frame is not swapped out");
        let kpage = frame.get_page_swap_in();
        addrspace
            .pagetable()
            .write()
            .mmap(frame.uaddr(), kpage, MapPerm::R | MapPerm::U);
    }

    fn push_buffer(
        &mut self,
        top: &mut usize,
        buffer: &[u8],
        pagetable: &mut PageTable,
        addrspace: &AddrSpace,
    ) -> SysResult<()> {
        let total_len = buffer.len();
        let new_top = *top - total_len;
        let mut uaddr = new_top;

        let mut copied = 0usize;
        let mut remaining = buffer.len();
        while remaining != 0 {
            let page_offset = uaddr & arch::PGMASK;
            let to_copy = core::cmp::min(arch::PGSIZE - page_offset, remaining);

            let page_index = (config::USER_STACK_TOP - uaddr - 1) / arch::PGSIZE;

            if self.frames[page_index].is_unallocated() {
                self.allocate_page(page_index, pagetable, addrspace);
            }

            match &self.frames[page_index] {
                FrameState::Allocated(frame) => {
                    let pa = frame.get_page_swap_in();
                    let dst = unsafe { core::slice::from_raw_parts_mut((pa + page_offset) as *mut u8, to_copy) };
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

    fn push_c_str(
        &mut self,
        top: &mut usize,
        s: &str,
        pagetable: &mut PageTable,
        addrspace: &AddrSpace,
    ) -> SysResult<()> {
        self.push_usize(top, 0, pagetable, addrspace)?;
        self.push_buffer(top, s.as_bytes(), pagetable, addrspace)
    }

    fn push_usize(
        &mut self,
        top: &mut usize,
        value: usize,
        pagetable: &mut PageTable,
        addrspace: &AddrSpace,
    ) -> SysResult<()> {
        // *top &= !(core::mem::size_of::<usize>() - 1);
        self.push_buffer(top, &value.to_le_bytes(), pagetable, addrspace)
    }

    fn push_auxv(
        &mut self,
        top: &mut usize,
        auxv: &Auxv,
        pagetable: &mut PageTable,
        addrspace: &AddrSpace,
    ) -> SysResult<()> {
        self.push_usize(top, 0, pagetable, addrspace)?;

        if auxv.length == 0 {
            return Ok(());
        }

        let buffer = unsafe {
            core::slice::from_raw_parts(
                auxv.auxv.as_ptr() as *const u8,
                auxv.length * 2 * core::mem::size_of::<usize>(),
            )
        };
        self.push_buffer(top, &buffer, pagetable, addrspace)?;

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
        let mut pagetable = addrspace.pagetable().lock();
        let mut top = config::USER_STACK_TOP;

        let mut envp_ptrs = Vec::with_capacity(envp.len());
        for &env in envp.iter() {
            self.push_c_str(&mut top, env, &mut pagetable, addrspace)?;
            envp_ptrs.push(top);
        }

        let mut argv_ptrs = Vec::with_capacity(argv.len());
        for &arg in argv.iter() {
            self.push_c_str(&mut top, arg, &mut pagetable, addrspace)?;
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
        self.push_auxv(&mut top, auxv, &mut pagetable, addrspace)?;

        // Push envp pointers
        self.push_usize(&mut top, 0, &mut pagetable, addrspace)?;
        for &addr in envp_ptrs.iter().rev() {
            self.push_usize(&mut top, addr, &mut pagetable, addrspace)?;
        }

        // Push argv pointers
        self.push_usize(&mut top, 0, &mut pagetable, addrspace)?;
        for &addr in argv_ptrs.iter().rev() {
            self.push_usize(&mut top, addr, &mut pagetable, addrspace)?;
        }

        // Push argc
        self.push_usize(&mut top, argv.len(), &mut pagetable, addrspace)?;

        debug_assert!(top % 16 == 0, "User stack top is not aligned to 16 bytes: {:#x}", top);

        Ok(top)
    }
}

impl Area for UserStack {
    fn translate_read(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize> {
        if uaddr >= config::USER_STACK_TOP {
            return None;
        }

        let page_index = (config::USER_STACK_TOP - uaddr - 1) / arch::PGSIZE;
        if page_index < self.get_max_page_count() {
            let page = match &self.frames[page_index] {
                FrameState::Unallocated => self.allocate_page(page_index, &mut addrspace.pagetable().lock(), addrspace),
                FrameState::Allocated(frame) | FrameState::Cow(frame) => frame.get_page_swap_in(),
            };

            Some(page + uaddr % arch::PGSIZE)
        } else {
            None
        }
    }

    fn translate_write(&mut self, uaddr: usize, addrspace: &AddrSpace) -> Option<usize> {
        if uaddr >= config::USER_STACK_TOP {
            return None;
        }

        let page_index = (config::USER_STACK_TOP - uaddr - 1) / arch::PGSIZE;
        if page_index < self.get_max_page_count() {
            let page = match &self.frames[page_index] {
                FrameState::Unallocated => {
                    let mut pagetable = addrspace.pagetable().lock();
                    self.allocate_page(page_index, &mut pagetable, addrspace)
                }
                FrameState::Allocated(frame) => frame.get_page_swap_in(),
                FrameState::Cow(_) => self.copy_on_write_page(page_index, addrspace),
            };

            Some(page + uaddr % arch::PGSIZE)
        } else {
            None
        }
    }

    fn perm(&self) -> MapPerm {
        MapPerm::R | MapPerm::W | MapPerm::U
    }

    fn fork(&mut self, self_pagetable: &SpinLock<PageTable>, new_pagetable: &mut PageTable) -> Box<dyn Area> {
        let new_frames = self
            .frames
            .iter()
            .enumerate()
            .map(|(page_index, frame)| match frame {
                FrameState::Unallocated => FrameState::Unallocated,
                FrameState::Allocated(frame) | FrameState::Cow(frame) => {
                    if let Some(kpage) = frame.get_page() {
                        new_pagetable.mmap(
                            config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE,
                            kpage,
                            MapPerm::R | MapPerm::U,
                        );
                    }
                    FrameState::Cow(frame.clone())
                }
            })
            .collect();

        let mut self_pagetable = self_pagetable.lock();
        self.frames.iter_mut().enumerate().for_each(|(index, frame)| {
            *frame = match frame {
                FrameState::Unallocated => FrameState::Unallocated,
                FrameState::Allocated(frame) | FrameState::Cow(frame) => {
                    if !frame.is_swapped_out() {
                        self_pagetable.mmap_replace_perm(
                            config::USER_STACK_TOP - (index + 1) * arch::PGSIZE,
                            MapPerm::R | MapPerm::U,
                        );
                    }
                    FrameState::Cow(frame.clone())
                }
            }
        });

        Box::new(UserStack { frames: new_frames })
    }

    fn try_to_fix_memory_fault(
        &mut self,
        addr: usize,
        access_type: MemAccessType,
        addrspace: &AddrSpace,
    ) -> Result<usize, MemoryFaultSignal> {
        if addr >= config::USER_STACK_TOP {
            return Err(MemoryFaultSignal::Segv);
        }

        let page_index = (config::USER_STACK_TOP - addr - 1) / arch::PGSIZE;

        if page_index >= self.get_max_page_count() {
            return Err(MemoryFaultSignal::Segv);
        }

        match &self.frames[page_index] {
            FrameState::Allocated(frame) => {
                #[cfg(feature = "swap-memory")]
                self.handle_memory_fault_on_swapped_allocated(frame, addrspace);
                #[cfg(not(feature = "swap-memory"))]
                {
                    let _ = frame;
                    // Maybe the page is allocated and mapped by the other task.
                }
            }
            FrameState::Cow(frame) => {
                if access_type != MemAccessType::Write {
                    // If it's a read fault on a COW page, it might be swapped out
                    #[cfg(feature = "swap-memory")]
                    self.handle_cow_read_swapped_out(frame, addrspace);
                    #[cfg(not(feature = "swap-memory"))]
                    {
                        let _ = frame;
                        panic!(
                            "Access type is not write for COW page at index {}, addr={:#x}, flags={:?}",
                            page_index,
                            addr,
                            addrspace.pagetable().lock().mapped_flag(addr)
                        );
                    }
                } else {
                    self.copy_on_write_page(page_index, addrspace);
                }
            }
            FrameState::Unallocated => {
                let mut pagetable = addrspace.pagetable().lock();
                self.allocate_page(page_index, &mut pagetable, addrspace);
            }
        }

        if access_type == MemAccessType::Write {
            self.translate_write(addr, addrspace).ok_or(MemoryFaultSignal::Segv)
        } else {
            self.translate_read(addr, addrspace).ok_or(MemoryFaultSignal::Segv)
        }
    }

    fn ubase(&self) -> usize {
        config::USER_STACK_TOP - self.get_max_page_count() * arch::PGSIZE
    }

    fn page_count(&self) -> usize {
        self.get_max_page_count()
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        let mut pagetable = pagetable.lock();
        for (page_index, frame) in self.frames.iter_mut().enumerate() {
            if !frame.is_unallocated() {
                let uaddr = config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE;

                #[cfg(feature = "swap-memory")]
                let is_mapped = match frame {
                    FrameState::Allocated(f) | FrameState::Cow(f) => !f.is_swapped_out(),
                    _ => false,
                };
                #[cfg(not(feature = "swap-memory"))]
                let is_mapped = true;
                if is_mapped {
                    pagetable.munmap(uaddr);
                }

                *frame = FrameState::Unallocated;
            }
        }
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
