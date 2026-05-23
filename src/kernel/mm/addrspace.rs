use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use fixedstr::tstr;
use spin::Lazy;

use crate::arch::{PageTable, PageTableTrait, UserContext, TRAMPOLINE_BASE};
use crate::kernel::config::{MAX_PATH_LEN, USER_BRK_BASE, USER_RANDOM_ADDR_BASE};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::Auxv;
use crate::kernel::mm::{maparea, PhysPageFrame};
use crate::klib::{SleepLock, SpinLock};
use crate::{arch, safe_page_write};

use super::{vdso, MapPerm, MemAccessType};

cfg_if::cfg_if! {
    if #[cfg(feature="swap-memory")] {
        use alloc::collections::LinkedList;
    }
}

unsafe extern "C" {
    static __trampoline_start: u8;
}

static RANDOM_PAGE: Lazy<PhysPageFrame> = Lazy::new(|| PhysPageFrame::alloc());

pub trait AddrSpaceWatcher: Send + Sync {
    fn on_addrspace_unmap(&self, uaddr: usize, page_count: usize);

    fn on_addrspace_remap(&self, uaddr: usize, page_count: usize) {
        self.on_addrspace_unmap(uaddr, page_count);
    }

    fn on_addrspace_perm_change(&self, uaddr: usize, page_count: usize, _perm: MapPerm) {
        self.on_addrspace_unmap(uaddr, page_count);
    }
}

fn create_pagetable() -> PageTable {
    let mut pagetable = PageTable::new();
    pagetable.create();
    pagetable.mmap(
        TRAMPOLINE_BASE,
        core::ptr::addr_of!(__trampoline_start) as usize,
        MapPerm::R | MapPerm::X,
    );
    pagetable.mmap(USER_RANDOM_ADDR_BASE, RANDOM_PAGE.get_page(), MapPerm::R | MapPerm::U);

    vdso::map_to_pagetale(&mut pagetable);

    pagetable
}

#[cfg(feature = "swap-memory")]
use crate::kernel::mm::swappable::AddrSpaceFamilyChain;

pub struct AddrSpace {
    map_manager: SleepLock<maparea::Manager>,
    pagetable: SpinLock<PageTable>,
    usercontext_frames: SpinLock<Vec<PhysPageFrame>>,
    watchers: SpinLock<Vec<Weak<dyn AddrSpaceWatcher>>>,

    #[cfg(feature = "swap-memory")]
    family_chain: AddrSpaceFamilyChain,
}

impl AddrSpace {
    pub fn new() -> Arc<Self> {
        Self::new_with_pagetable(create_pagetable())
    }

    pub fn new_with_pagetable(pagetable: PageTable) -> Arc<Self> {
        let addrspace = Arc::new(AddrSpace {
            map_manager: SleepLock::new(maparea::Manager::new(), "AddrSpace::map_manager"),
            pagetable: SpinLock::new(pagetable, "AddrSpace::pagetable"),
            usercontext_frames: SpinLock::new(Vec::new(), "AddrSpace::usercontext_frames"),
            watchers: SpinLock::new(Vec::new(), "AddrSpace::watchers"),

            #[cfg(feature = "swap-memory")]
            family_chain: AddrSpaceFamilyChain::new(SpinLock::new(LinkedList::new(), "AddrSpace::family_chain")),
        });

        #[cfg(feature = "swap-memory")]
        addrspace.family_chain.lock().push_back(Arc::downgrade(&addrspace));

        addrspace
    }

    pub fn fork(self: &Arc<Self>) -> Arc<AddrSpace> {
        let mut new_pagetable = create_pagetable();

        let new_map_manager = self.map_manager.lock().fork(&self.pagetable, &mut new_pagetable);

        let addrspace = Arc::new(AddrSpace {
            map_manager: SleepLock::new(new_map_manager, "AddrSpace::map_manager"),
            pagetable: SpinLock::new(new_pagetable, "AddrSpace::pagetable"),
            usercontext_frames: SpinLock::new(Vec::new(), "AddrSpace::usercontext_frames"),
            watchers: SpinLock::new(Vec::new(), "AddrSpace::watchers"),

            #[cfg(feature = "swap-memory")]
            family_chain: AddrSpaceFamilyChain::new(SpinLock::new(LinkedList::new(), "AddrSpace::family_chain")),
        });

        #[cfg(feature = "swap-memory")]
        {
            let weak = Arc::downgrade(&addrspace);
            addrspace.family_chain.lock().push_back(weak.clone());
            self.family_chain.lock().push_back(weak);
        }

        addrspace
    }

    #[cfg(feature = "swap-memory")]
    pub fn family_chain(&self) -> &AddrSpaceFamilyChain {
        &self.family_chain
    }

    pub fn alloc_usercontext_page(&self) -> (usize, *mut UserContext) {
        let mut frames = self.usercontext_frames.lock();
        let frame = PhysPageFrame::alloc_zeroed();

        let uaddr = TRAMPOLINE_BASE - (frames.len() + 1) * arch::PGSIZE;
        let kaddr = frame.get_page();
        let user_context_ptr = kaddr as *mut UserContext;

        // Map the user context page in the pagetable
        // self.pagetable.write().mmap(uaddr, kaddr, MapPerm::R | MapPerm::W);
        self.pagetable.lock().mmap(uaddr, kaddr, MapPerm::R | MapPerm::W);

        frames.push(frame);

        (uaddr, user_context_ptr)
    }

    pub fn create_user_stack(&self, argv: &[&str], envp: &[&str], auxv: &Auxv) -> Result<usize, Errno> {
        // self.user_stack.create(argv, envp, aux, &mut self.map_manager)
        let mut map_manager = self.map_manager.lock();
        map_manager.create_user_stack(argv, envp, auxv, self)
    }

    pub fn map_area(&self, uaddr: usize, area: Box<dyn maparea::Area>) -> Result<(), Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.map_area(uaddr, area);

        Ok(())
    }

    pub fn map_area_fixed(&self, uaddr: usize, area: Box<dyn maparea::Area>) -> Result<(), Errno> {
        let page_count = area.page_count();
        {
            let mut map_manager = self.map_manager.lock();
            map_manager.map_area_fixed(uaddr, area, &self.pagetable);
        }
        self.notify_addrspace_remap(uaddr, page_count);

        Ok(())
    }

    pub fn unmap_area(&self, uaddr: usize, page_count: usize) -> Result<(), Errno> {
        {
            let mut map_manager = self.map_manager.lock();
            map_manager.unmap_area(uaddr, page_count, &self.pagetable)?;
        }
        self.notify_addrspace_unmap(uaddr, page_count);

        Ok(())
    }

    pub fn set_area_perm(&self, uaddr: usize, page_count: usize, perm: MapPerm) -> Result<(), Errno> {
        {
            let mut map_manager = self.map_manager.lock();
            map_manager.set_map_area_perm(uaddr, page_count, perm, &self.pagetable)?;
        }
        self.notify_addrspace_perm_change(uaddr, page_count, perm);

        Ok(())
    }

    pub fn increase_userbrk(&self, ubrk: usize) -> Result<usize, Errno> {
        let old_page_count = self.map_manager.lock().userbrk_page_count();
        let new_ubrk = {
            let mut map_manager = self.map_manager.lock();
            map_manager.increase_userbrk(ubrk, &self.pagetable)?
        };
        let new_page_count = self.map_manager.lock().userbrk_page_count();
        if new_page_count < old_page_count {
            self.notify_addrspace_unmap(
                USER_BRK_BASE + new_page_count * arch::PGSIZE,
                old_page_count - new_page_count,
            );
        }

        Ok(new_ubrk)
    }

    pub fn translate_write(self: &Arc<Self>, uaddr: usize) -> SysResult<usize> {
        self.map_manager
            .lock()
            .translate_write(uaddr, self)
            .ok_or(Errno::EFAULT)
    }

    pub fn copy_to_user_buffer(&self, mut uaddr: usize, buffer: &[u8]) -> Result<(), Errno> {
        let mut left = buffer.len();
        let mut copied: usize = 0;

        let mut map_manager = self.map_manager.lock();

        while left > 0 {
            let kaddr = map_manager.translate_write(uaddr, self).ok_or(Errno::EFAULT)?;

            let page_offset = uaddr & (arch::PGSIZE - 1);
            let write_len = core::cmp::min(left, arch::PGSIZE - page_offset);

            safe_page_write!(kaddr, &buffer[copied..copied + write_len]);

            copied += write_len;
            left -= write_len;
            uaddr += write_len;
        }

        Ok(())
    }

    pub fn copy_to_user<T: Copy>(&self, uaddr: usize, value: T) -> Result<(), Errno> {
        let buffer =
            unsafe { core::slice::from_raw_parts((&value as *const T) as *const u8, core::mem::size_of::<T>()) };
        self.copy_to_user_buffer(uaddr, buffer)
    }

    /// Copy a slice to user space
    pub fn copy_to_user_slice<T>(&self, uaddr: usize, slice: &[T]) -> SysResult<()> {
        let buffer = unsafe { core::slice::from_raw_parts(slice.as_ptr() as *const u8, core::mem::size_of_val(slice)) };
        self.copy_to_user_buffer(uaddr, buffer)
    }

    pub fn copy_from_user_buffer(&self, mut uaddr: usize, buffer: &mut [u8]) -> Result<(), Errno> {
        let mut left = buffer.len();
        let mut copied: usize = 0;

        let mut map_manager = self.map_manager.lock();

        while left > 0 {
            let kaddr = map_manager.translate_read(uaddr, self).ok_or(Errno::EFAULT)?;

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

    pub fn copy_from_user<T: Copy>(&self, uaddr: usize) -> Result<T, Errno> {
        let mut value: T = unsafe { core::mem::zeroed() };
        let buffer =
            unsafe { core::slice::from_raw_parts_mut(&mut value as *mut T as *mut u8, core::mem::size_of::<T>()) };
        self.copy_from_user_buffer(uaddr, buffer)?;
        Ok(value)
    }

    pub fn get_user_string(&self, mut uaddr: usize) -> Result<tstr<255>, Errno> {
        self.get_user_tstr_limited(&mut uaddr, Errno::ENAMETOOLONG)
    }

    pub fn get_user_string_fixed<const N: usize>(&self, mut uaddr: usize) -> Result<tstr<N>, Errno> {
        self.get_user_tstr_limited(&mut uaddr, Errno::ENAMETOOLONG)
    }

    pub fn get_user_path_string(&self, mut uaddr: usize) -> Result<String, Errno> {
        self.get_user_string_limited(&mut uaddr, MAX_PATH_LEN, Errno::ENAMETOOLONG)
    }

    fn get_user_tstr_limited<const N: usize>(
        &self,
        uaddr: &mut usize,
        too_long_errno: Errno,
    ) -> Result<tstr<N>, Errno> {
        if N == 0 || N > 256 {
            return Err(Errno::EINVAL);
        }

        let max_size = N - 1;
        let mut map_manager = self.map_manager.lock();
        let mut result = tstr::<N>::new();

        loop {
            let page_offset = *uaddr & arch::PGMASK;
            let to_read = arch::PGSIZE - page_offset;
            let kaddr = map_manager.translate_read(*uaddr, self).ok_or(Errno::EFAULT)?;

            let slice = unsafe { core::slice::from_raw_parts(kaddr as *const u8, to_read) };
            let (bytes, done) = match slice.iter().position(|&b| b == 0) {
                Some(pos) => (&slice[..pos], true),
                None => (slice, false),
            };

            let part = core::str::from_utf8(bytes).map_err(|_| Errno::EINVAL)?;
            if result.len() + part.len() > max_size {
                return Err(too_long_errno);
            }

            if !result.push_str(part).is_empty() {
                return Err(too_long_errno);
            }

            if done {
                break;
            }

            *uaddr += to_read;
        }

        Ok(result)
    }

    fn get_user_string_limited(
        &self,
        uaddr: &mut usize,
        max_size: usize,
        too_long_errno: Errno,
    ) -> Result<String, Errno> {
        let mut map_manager = self.map_manager.lock();

        let mut result = String::new();

        loop {
            let page_offset = *uaddr & arch::PGMASK;
            let to_read = arch::PGSIZE - page_offset;
            let kaddr = map_manager.translate_read(*uaddr, self).ok_or(Errno::EFAULT)?;

            let slice = unsafe { core::slice::from_raw_parts(kaddr as *const u8, to_read) };
            if let Some(pos) = slice.iter().position(|&b| b == 0) {
                result.push_str(&String::from_utf8(slice[..pos].to_vec()).map_err(|_| Errno::EINVAL)?);
                break;
            } else {
                result.push_str(&String::from_utf8(slice.to_vec()).map_err(|_| Errno::EINVAL)?);
                if result.len() > max_size {
                    return Err(too_long_errno);
                }
            }

            *uaddr += to_read;
        }

        if result.len() > max_size {
            return Err(too_long_errno);
        }

        Ok(result)
    }

    pub fn copy_from_user_slice<T: Copy>(&self, uaddr: usize, slice: &mut [T]) -> SysResult<()> {
        let buffer =
            unsafe { core::slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut u8, core::mem::size_of_val(slice)) };
        self.copy_from_user_buffer(uaddr, buffer)
    }

    pub fn with_pagetable<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&PageTable) -> R,
    {
        // f(&self.pagetable.read())
        f(&self.pagetable.lock())
    }

    pub fn pagetable(&self) -> &SpinLock<PageTable> {
        &self.pagetable
    }

    pub fn add_watcher(&self, watcher: Weak<dyn AddrSpaceWatcher>) {
        self.watchers.lock().push(watcher);
    }

    fn live_watchers(&self) -> Vec<Arc<dyn AddrSpaceWatcher>> {
        let mut live = Vec::new();
        self.watchers.lock().retain(|watcher| {
            if let Some(watcher) = watcher.upgrade() {
                live.push(watcher);
                true
            } else {
                false
            }
        });
        live
    }

    fn notify_addrspace_unmap(&self, uaddr: usize, page_count: usize) {
        if page_count == 0 {
            return;
        }
        for watcher in self.live_watchers() {
            watcher.on_addrspace_unmap(uaddr, page_count);
        }
    }

    fn notify_addrspace_remap(&self, uaddr: usize, page_count: usize) {
        if page_count == 0 {
            return;
        }
        for watcher in self.live_watchers() {
            watcher.on_addrspace_remap(uaddr, page_count);
        }
    }

    fn notify_addrspace_perm_change(&self, uaddr: usize, page_count: usize, perm: MapPerm) {
        if page_count == 0 {
            return;
        }
        for watcher in self.live_watchers() {
            watcher.on_addrspace_perm_change(uaddr, page_count, perm);
        }
    }

    pub fn with_map_manager_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut maparea::Manager) -> R,
    {
        f(&mut self.map_manager.lock())
    }

    pub fn try_to_fix_memory_fault(
        self: &Arc<Self>,
        uaddr: usize,
        access_type: MemAccessType,
    ) -> Result<usize, maparea::MemoryFaultSignal> {
        let map_manager = &mut self.map_manager.lock();
        map_manager.try_to_fix_memory_fault(uaddr, access_type, self)
    }

    pub fn cleanup(&self) {
        // let pagetable = &mut self.pagetable.write();
        let mut map_manager = self.map_manager.lock();
        map_manager.cleanup();
    }

    #[cfg(feature = "swap-memory")]
    pub fn unmap_swap_page(&self, uaddr: usize, kaddr: usize) {
        self.pagetable.write().munmap_with_check(uaddr, kaddr);
    }

    #[cfg(feature = "swap-memory")]
    pub fn take_page_access_dirty_bit(&self, uaddr: usize) -> Option<(bool, bool)> {
        self.pagetable.write().take_access_dirty_bit(uaddr)
    }
}

impl Drop for AddrSpace {
    fn drop(&mut self) {
        let frames = self.usercontext_frames.lock();
        let mut pagetable = self.pagetable.lock();
        for i in 0..frames.len() {
            let uaddr = TRAMPOLINE_BASE - (i + 1) * arch::PGSIZE;
            pagetable.munmap(uaddr);
        }
    }
}

unsafe impl Send for AddrSpace {}
