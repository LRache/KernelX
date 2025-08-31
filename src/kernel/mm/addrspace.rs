use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use spin::{Lazy, Mutex, RwLock};

use crate::arch::{PageTable, PageTableTrait, UserContext};
use crate::arch;
use crate::safe_page_write;
use crate::kernel::errno::Errno;
use crate::kernel::mm::{maparea, PhysPageFrame};
use crate::kernel::mm::maparea::Auxv;
use crate::kernel::config::USER_RANDOM_ADDR_BASE;
use crate::platform::config::TRAMPOLINE_BASE;

use super::{MemAccessType, MapPerm};

unsafe extern "C"{
    static __trampoline_start: u8;
}

static RANDOM_PAGE: Lazy<PhysPageFrame> = Lazy::new(|| {
    PhysPageFrame::new()
});

fn create_pagetable() -> PageTable {
    let mut pagetable = PageTable::new();
    pagetable.create();
    pagetable.mmap(
        TRAMPOLINE_BASE, 
        core::ptr::addr_of!(__trampoline_start) as usize, 
        MapPerm::R | MapPerm::X
    );
    pagetable.mmap(
        USER_RANDOM_ADDR_BASE,
        RANDOM_PAGE.get_page(),
        MapPerm::R | MapPerm::U
    );
    pagetable
}

pub struct AddrSpace {
    map_manager: Mutex<maparea::Manager>,
    pagetable: RwLock<PageTable>,
    usercontext_frames: Mutex<Vec<PhysPageFrame>>,
}

impl AddrSpace {
    pub fn new() -> Self {        
        AddrSpace {
            map_manager: Mutex::new(maparea::Manager::new()),
            pagetable: RwLock::new(create_pagetable()),
            usercontext_frames: Mutex::new(Vec::new()),
        }
    }

    
    pub fn fork(&self) -> AddrSpace {
        let new_pagetable = RwLock::new(create_pagetable());

        let new_map_manager = self.map_manager.lock().fork(&self.pagetable, &new_pagetable);
        
        AddrSpace {
            map_manager: Mutex::new(new_map_manager),
            pagetable: new_pagetable,
            usercontext_frames: Mutex::new(Vec::new()),
        }
    }

    pub fn alloc_usercontext_page(&self) -> (usize, *mut UserContext) {
        let mut frames = self.usercontext_frames.lock();
        let frame = PhysPageFrame::new();
        
        let uaddr = TRAMPOLINE_BASE - (frames.len() + 1) * arch::PGSIZE;
        let kaddr = frame.get_page();
        let user_context_ptr = kaddr as *mut UserContext;

        // Map the user context page in the pagetable
        self.pagetable.write().mmap(uaddr, kaddr, MapPerm::R | MapPerm::W);

        frames.push(frame);

        (uaddr, user_context_ptr)
    }

    pub fn create_user_stack(&self, argv: &[&str], envp: &[&str], auxv: &Auxv) -> Result<usize, Errno> {
        // self.user_stack.create(argv, envp, aux, &mut self.map_manager)
        let mut map_manager = self.map_manager.lock();
        map_manager.create_user_stack(argv, envp, auxv, &self.pagetable)
    }

    pub fn map_area(&self, uaddr: usize, area: Box<dyn maparea::Area>) -> Result<(), Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.map_area(uaddr, area);

        Ok(())
    }

    pub fn set_area_perm(&self, uaddr: usize, page_count: usize, perm: MapPerm) -> Result<(), Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.set_map_area_perm(uaddr, page_count, perm, &self.pagetable)
    }

    pub fn increase_userbrk(&self, ubrk: usize) -> Result<usize, Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.increase_userbrk(ubrk)
    }

    pub fn copy_to_user(&self, mut uaddr: usize, buffer: &[u8]) -> Result<(), Errno> {
        let mut left = buffer.len();
        let mut copied: usize = 0;

        let mut map_manager = self.map_manager.lock();

        while left > 0 {
            let kaddr = map_manager.translate_write(uaddr, &self.pagetable).ok_or(Errno::EFAULT)?;
            
            let page_offset = uaddr & (arch::PGSIZE - 1);
            let write_len = core::cmp::min(left, arch::PGSIZE - page_offset);
            
            safe_page_write!(kaddr, &buffer[copied..copied + write_len]);

            copied += write_len;
            left -= write_len;
            uaddr += write_len;
        }

        Ok(())
    }

    pub fn copy_from_user(&self, mut uaddr: usize, buffer: &mut [u8]) -> Result<(), Errno> {
        let mut left = buffer.len();
        let mut copied: usize = 0;

        let mut map_manager = self.map_manager.lock();

        while left > 0 {
            let kaddr = map_manager.translate_read(uaddr, &self.pagetable).ok_or(Errno::EFAULT)?;

            let page_offset = uaddr & (arch::PGSIZE - 1);
            let read_len = core::cmp::min(left, arch::PGSIZE - page_offset);
            
            let src = unsafe { core::slice::from_raw_parts(kaddr as *const u8, read_len) };
            buffer[copied..copied + read_len].copy_from_slice(src);
            
            copied += read_len;
            left -= read_len;
            uaddr += read_len;
        }

        Ok(())
    }

    pub fn get_user_string(&self, mut uaddr: usize) -> Result<String, Errno> {
        let mut map_manager = self.map_manager.lock();

        let mut result = String::new();

        loop {
            let page_offset = uaddr & arch::PGMASK;
            let to_read = arch::PGSIZE - page_offset;
            let kaddr = map_manager.translate_read(uaddr, &self.pagetable).ok_or(Errno::EFAULT)?;

            let slice = unsafe { core::slice::from_raw_parts(kaddr as *const u8, to_read) };
            if let Some(pos) = slice.iter().position(|&b| b == 0) {
                result.push_str(&String::from_utf8(slice[..pos].to_vec()).map_err(|_| Errno::EINVAL)?);
                break;
            } else {
                result.push_str(&String::from_utf8(slice.to_vec()).map_err(|_| Errno::EINVAL)?);
            }

            uaddr += to_read;
        }

        Ok(result)
    }

    pub fn with_pagetable<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&PageTable) -> R,
    {
        f(&self.pagetable.read())
    }

    pub fn with_map_manager_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut maparea::Manager) -> R,
    {
        f(&mut self.map_manager.lock())
    }

    pub fn try_to_fix_memory_fault(&self, uaddr: usize, access_type: MemAccessType) -> bool {
        if !self.map_manager.lock().try_to_fix_memory_fault(uaddr, access_type, &self.pagetable) {
            self.map_manager.lock().print_all_areas();
            false
        } else {
            true
        }
    }

    pub fn set_map_area_perm(&self, uaddr: usize, page_count: usize, perm: MapPerm) -> Result<(), Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.set_map_area_perm(uaddr, page_count, perm, &self.pagetable)
    }
}
