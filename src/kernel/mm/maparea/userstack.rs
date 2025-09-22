use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::RwLock;

use crate::{arch, ktrace};
use crate::kernel::config;
use crate::kernel::mm::maparea::area::Area;
use crate::kernel::mm::{MemAccessType, PhysPageFrame};
use crate::kernel::mm::MapPerm;
use crate::kernel::errno::Errno;
use crate::arch::{PageTable, PageTableTrait};

use super::area::Frame;

pub enum AuxKey {
    _NULL   = 0,
    _IGNORE = 1,
    _EXECFD = 2,
    PHDR   = 3,
    PHENT  = 4,
    PHNUM  = 5,
    PAGESZ = 6,
    BASE   = 7,
    FLAGS  = 8,
    ENTRY  = 9,
    _NOTELF = 10,
    _UID    = 11,
    _EUID   = 12,
    _GID    = 13,
    _EGID   = 14,
    RANDOM  = 25,
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
        assert!(self.length < AUX_MAX, "Auxiliary vector is full, cannot add more entries.");
        self.auxv[self.length * 2] = key as usize;
        self.auxv[self.length * 2 + 1] = value;
        self.length += 1;
    }
}

pub struct UserStack {
    frames: Vec<Frame>,
}

impl UserStack {
    pub fn new() -> Self {
        let frames = Vec::from_iter((0..config::USER_STACK_PAGE_COUNT_MAX).map(|_| Frame::Unallocated));
        Self {
            frames,
        }
    }
    
    fn get_max_page_count(&self) -> usize {
        self.frames.len()
    }

    fn allocate_page(&mut self, page_index: usize, pagetable: &mut PageTable) -> usize {
        assert!(page_index < self.get_max_page_count(), "Page index out of bounds: {}", page_index);
        
        assert!(self.frames[page_index].is_unallocated(), "Page at index {} is already allocated", page_index);
        
        let new_frame = PhysPageFrame::new();
        pagetable.mmap(
            config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE,
            new_frame.get_page(),
            MapPerm::R | MapPerm::W | MapPerm::U,
        );

        let page = new_frame.get_page();

        self.frames[page_index] = Frame::Allocated(Arc::new(new_frame));

        page
    }

    fn copy_on_write_page(&mut self, page_index: usize, pagetable: &mut PageTable) -> usize {
        assert!(page_index < self.get_max_page_count(), "Page index out of bounds: {}", page_index);
        assert!(self.frames[page_index].is_cow(), "Page at index {} is not allocated", page_index);

        let frame = core::mem::replace(&mut self.frames[page_index], Frame::Unallocated);
        
        let new_frame = 
        if let Frame::Cow(arc_frame) = frame {
            match Arc::try_unwrap(arc_frame) {
                Ok(frame) => {
                    frame
                }
                Err(cow_frame) => {
                    cow_frame.copy()
                }
            }
        } else {
            unreachable!();
        };

        let new_page = new_frame.get_page();
        self.frames[page_index] = Frame::Allocated(Arc::new(new_frame));
        
        pagetable.mmap_replace(
            config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE,
            new_page,
            MapPerm::R | MapPerm::W | MapPerm::U,
        );
        
        new_page
    }

    fn push_buffer(&mut self, top: &mut usize, buffer: &[u8], pagetable: &mut PageTable) -> Result<(), Errno> {
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
                self.allocate_page(page_index, pagetable);
            }
            
            match &self.frames[page_index] {
                Frame::Allocated(frame) => {
                    frame.copy_from_slice(page_offset, &buffer[copied..copied + to_copy]);
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

    fn push_c_str(&mut self, top: &mut usize, s: &str, pagetable: &mut PageTable) -> Result<(), Errno> {
        self.push_usize(top, 0, pagetable)?;
        self.push_buffer(top, s.as_bytes(), pagetable)
    }

    fn push_usize(&mut self, top: &mut usize, value: usize, pagetable: &mut PageTable) -> Result<(), Errno> {
        self.push_buffer(top, &value.to_le_bytes(), pagetable)
    }

    fn push_auxv(&mut self, top: &mut usize, auxv: &Auxv, pagetable: &mut PageTable) -> Result<(), Errno> {
        self.push_usize(top, 0, pagetable)?;

        if auxv.length == 0 {
            return Ok(());
        }
        
        let buffer = unsafe {
            core::slice::from_raw_parts(auxv.auxv.as_ptr() as *const u8, auxv.length * 2 * core::mem::size_of::<usize>())
        };
        self.push_buffer(top, &buffer, pagetable)?;

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
    pub fn push_argv_envp_auxv(&mut self, argv: &[&str], envp: &[&str], auxv: &Auxv, pagetable: &RwLock<PageTable>) -> Result<usize, Errno> {
        let mut pagetable = pagetable.write();
        let mut top = config::USER_STACK_TOP;
        
        let mut envp_ptrs = Vec::with_capacity(envp.len());
        for &env in envp.iter() {
            self.push_c_str(&mut top, env, &mut pagetable)?;
            envp_ptrs.push(top);
        }
        
        let mut argv_ptrs = Vec::with_capacity(argv.len());
        for &arg in argv.iter() {
            self.push_c_str(&mut top, arg, &mut pagetable)?;
            argv_ptrs.push(top);
        }

        self.push_auxv(&mut top, auxv, &mut pagetable)?;

        self.push_usize(&mut top, 0, &mut pagetable)?;
        for &addr in envp_ptrs.iter().rev() {
            self.push_usize(&mut top, addr, &mut pagetable)?;
            ktrace!("push envp pointer: {:#x}", addr);
        }

        self.push_usize(&mut top, 0, &mut pagetable)?;
        for &addr in argv_ptrs.iter().rev() {
            self.push_usize(&mut top, addr, &mut pagetable)?;
        }

        self.push_usize(&mut top, argv.len(), &mut pagetable)?;

        Ok(top)
    }
}

impl Area for UserStack {
    fn translate_read(&mut self, vaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize> {
        let page_index = (config::USER_STACK_TOP - vaddr - 1) / arch::PGSIZE;
        if page_index < self.get_max_page_count() {            
            let page = match &self.frames[page_index] {
                Frame::Unallocated => {
                    self.allocate_page(page_index, &mut pagetable.write())
                }
                Frame::Cow(_) => {
                    self.copy_on_write_page(page_index, &mut pagetable.write())
                }
                Frame::Allocated(frame) => frame.get_page(),
            };
            
            Some(page + vaddr % arch::PGSIZE)
        } else {
            None
        }
    }

    fn translate_write(&mut self, vaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize> {
        let page_index = (config::USER_STACK_TOP - vaddr - 1) / arch::PGSIZE;
        if page_index < self.get_max_page_count() {
            let page = match &self.frames[page_index] {
                Frame::Unallocated => {
                    let mut pagetable = pagetable.write();
                    self.allocate_page(page_index, &mut pagetable)
                }
                Frame::Allocated(frame) => frame.get_page(),
                Frame::Cow(_) => {
                    self.copy_on_write_page(page_index, &mut pagetable.write())
                }
            };
            
            Some(page + vaddr % arch::PGSIZE)
        } else {
            None
        }
    }

    fn fork(&mut self, self_pagetable: &RwLock<PageTable>, new_pagetable: &RwLock<PageTable>) -> Box<dyn Area> {
        let mut new_pagetable = new_pagetable.write();
        
        let new_frames = self.frames.iter().enumerate().map(|(page_index, frame)| {
            match frame {
                Frame::Unallocated => Frame::Unallocated,
                Frame::Allocated(frame) | Frame::Cow(frame) => {
                    new_pagetable.mmap(
                        config::USER_STACK_TOP - (page_index + 1) * arch::PGSIZE,
                        frame.get_page(), 
                        MapPerm::R | MapPerm::U
                    );
                    Frame::Cow(frame.clone())
                }
            }
        }).collect();

        let mut self_pagetable = self_pagetable.write();
        self.frames.iter_mut().enumerate().for_each(|(index, frame)| {
            *frame = match frame {
                Frame::Unallocated => {
                    Frame::Unallocated
                }
                Frame::Allocated(frame) | Frame::Cow(frame) => {
                    self_pagetable.mmap_replace(
                        config::USER_STACK_TOP - (index + 1) * arch::PGSIZE,
                        frame.get_page(),
                        MapPerm::R | MapPerm::U
                    );
                    Frame::Cow(frame.clone())
                }
            }
        });

        Box::new(UserStack {
            frames: new_frames,
        })
    }

    fn try_to_fix_memory_fault(&mut self, addr: usize, access_type: MemAccessType, pagetable: &RwLock<PageTable>) -> bool {
        ktrace!("UserStack::try_to_fix_memory_fault: addr={:#x}, access_type={:?}, tid={}", addr, access_type, crate::kernel::scheduler::current::tid());
        
        if addr >= config::USER_STACK_TOP {
            return false;
        }

        if access_type == MemAccessType::Execute {
            return false;
        }

        let page_index = (config::USER_STACK_TOP - addr) / arch::PGSIZE;

        if page_index >= self.get_max_page_count() {
            return false;
        }

        match &self.frames[page_index] {
            Frame::Allocated(_) => {
                panic!("Page at index {} is already allocated", page_index);
            }
            Frame::Cow(_) => {
                let mut pagetable = pagetable.write();
                self.copy_on_write_page(page_index, &mut pagetable);
            }
            Frame::Unallocated => {
                let mut pagetable = pagetable.write();
                self.allocate_page(page_index, &mut pagetable);
            }
        }
        
        true
    }

    fn page_count(&self) -> usize {
        self.get_max_page_count()
    }

    fn split(&mut self, uaddr: usize) -> Box<dyn Area> {
        assert!(uaddr < config::USER_STACK_TOP, "Split address out of bounds: {:#x}", uaddr);
        
        let split_index = (config::USER_STACK_TOP - uaddr + arch::PGSIZE - 1) / arch::PGSIZE;
        assert!(split_index <= self.get_max_page_count(), "Split index out of bounds: {}", split_index);

        let new_frames = self.frames.split_off(split_index);
        Box::new(UserStack {
            frames: new_frames,
        })
    }

    fn set_perm(&mut self, _perm: MapPerm, _pagetable: &RwLock<PageTable>) {
        // UserStack has fixed permissions (read and write)
        // Permission changes are not allowed for user stack
        // This is a no-op to satisfy the trait requirement
        ktrace!("Attempted to change permissions on user stack - ignored");
    }

    fn type_name(&self) -> &'static str {
        "UserStack"
    }
}
