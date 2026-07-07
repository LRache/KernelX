use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use fixedstr::tstr;
use spin::Lazy;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::kernel::config::{MAX_PATH_LEN, USER_RANDOM_ADDR_BASE};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::{Auxv, MapManagerWatcher};
use crate::kernel::mm::{PhysPageFrame, maparea};
use crate::klib::{SleepLock, SpinLock};

use super::{MapPerm, MemAccessType, vdso};

cfg_if::cfg_if! {
    if #[cfg(feature="swap-memory")] {
        use alloc::collections::LinkedList;
    }
}

static RANDOM_PAGE: Lazy<Arc<PhysPageFrame>> = Lazy::new(|| Arc::new(PhysPageFrame::alloc()));

fn create_pagetable() -> PageTable {
    let mut pagetable = PageTable::new_user();
    pagetable.mmap(USER_RANDOM_ADDR_BASE, &RANDOM_PAGE, MapPerm::R | MapPerm::U);

    vdso::map_to_pagetale(&mut pagetable);

    pagetable
}

#[cfg(feature = "swap-memory")]
use crate::kernel::mm::swappable::AddrSpaceFamilyChain;

pub struct AddrSpace {
    map_manager: SleepLock<maparea::Manager>,
    pagetable: SpinLock<PageTable>,

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

            #[cfg(feature = "swap-memory")]
            family_chain: AddrSpaceFamilyChain::new(SpinLock::new(LinkedList::new(), "AddrSpace::family_chain")),
        });

        #[cfg(feature = "swap-memory")]
        addrspace.family_chain.lock().push_back(Arc::downgrade(&addrspace));

        addrspace
    }

    pub fn fork(self: &Arc<Self>) -> Arc<AddrSpace> {
        let new_pagetable = create_pagetable();

        let new_map_manager = self.map_manager.lock().fork(&self.pagetable);

        let addrspace = Arc::new(AddrSpace {
            map_manager: SleepLock::new(new_map_manager, "AddrSpace::map_manager"),
            pagetable: SpinLock::new(new_pagetable, "AddrSpace::pagetable"),

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

    pub fn create_user_stack(&self, argv: &[&str], envp: &[&str], auxv: &Auxv) -> Result<usize, Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.create_user_stack(argv, envp, auxv, self)
    }

    pub fn map_area(&self, uaddr: usize, area: Box<dyn maparea::Area>) -> Result<(), Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.map_area(uaddr, area);

        Ok(())
    }

    pub fn map_area_fixed(&self, uaddr: usize, area: Box<dyn maparea::Area>) -> Result<(), Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.map_area_fixed(uaddr, area, &self.pagetable);

        Ok(())
    }

    pub fn unmap_area(&self, uaddr: usize, page_count: usize) -> Result<(), Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.unmap_area(uaddr, page_count, &self.pagetable)?;

        Ok(())
    }

    pub fn set_area_perm(&self, uaddr: usize, page_count: usize, perm: MapPerm) -> Result<(), Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.set_map_area_perm(uaddr, page_count, perm, &self.pagetable)?;

        Ok(())
    }

    pub fn increase_userbrk(&self, ubrk: usize) -> Result<usize, Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.increase_userbrk(ubrk, &self.pagetable)
    }

    pub fn translate_write(self: &Arc<Self>, uaddr: usize) -> SysResult<usize> {
        self.map_manager
            .lock()
            .translate_write(uaddr, self)
            .ok_or(Errno::EFAULT)
    }

    pub fn translate_read(self: &Arc<Self>, uaddr: usize) -> SysResult<usize> {
        self.map_manager.lock().translate_read(uaddr, self).ok_or(Errno::EFAULT)
    }

    pub fn with_translated_read<F, R>(&self, uaddr: usize, len: usize, f: F) -> SysResult<R>
    where
        F: FnOnce(&[u8]) -> R,
    {
        if len == 0 {
            return Ok(f(&[]));
        }
        if len > arch::PGSIZE - (uaddr & arch::PGMASK) {
            return Err(Errno::EFAULT);
        }

        let mut map_manager = self.map_manager.lock();
        let kaddr = map_manager.translate_read(uaddr, self).ok_or(Errno::EFAULT)?;
        if len > arch::PGSIZE - (kaddr & arch::PGMASK) {
            return Err(Errno::EFAULT);
        }

        // SAFETY: The translated range is confined to one page, and the
        // MapManager lock keeps the backing MapArea/PageFrame alive while
        // the slice is consumed by the closure.
        let slice = unsafe { core::slice::from_raw_parts(kaddr as *const u8, len) };
        Ok(f(slice))
    }

    pub fn with_translated_write<F, R>(&self, uaddr: usize, len: usize, f: F) -> SysResult<R>
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        if len == 0 {
            return Ok(f(&mut []));
        }
        if len > arch::PGSIZE - (uaddr & arch::PGMASK) {
            return Err(Errno::EFAULT);
        }

        let mut map_manager = self.map_manager.lock();
        let kaddr = map_manager.translate_write(uaddr, self).ok_or(Errno::EFAULT)?;
        if len > arch::PGSIZE - (kaddr & arch::PGMASK) {
            return Err(Errno::EFAULT);
        }

        // SAFETY: The translated range is confined to one page, and the
        // MapManager lock keeps the backing MapArea/PageFrame alive while
        // the mutable slice is consumed by the closure.
        let slice = unsafe { core::slice::from_raw_parts_mut(kaddr as *mut u8, len) };
        Ok(f(slice))
    }

    pub fn with_translated_read_write<F, R>(
        &self,
        read_uaddr: usize,
        write_uaddr: usize,
        len: usize,
        f: F,
    ) -> SysResult<R>
    where
        F: FnOnce(&[u8], &mut [u8]) -> R,
    {
        if len == 0 {
            return Ok(f(&[], &mut []));
        }
        if len > arch::PGSIZE - (read_uaddr & arch::PGMASK) || len > arch::PGSIZE - (write_uaddr & arch::PGMASK) {
            return Err(Errno::EFAULT);
        }

        let mut map_manager = self.map_manager.lock();
        let read_kaddr = map_manager.translate_read(read_uaddr, self).ok_or(Errno::EFAULT)?;
        let write_kaddr = map_manager.translate_write(write_uaddr, self).ok_or(Errno::EFAULT)?;
        if len > arch::PGSIZE - (read_kaddr & arch::PGMASK) || len > arch::PGSIZE - (write_kaddr & arch::PGMASK) {
            return Err(Errno::EFAULT);
        }
        let read_end = read_kaddr.checked_add(len).ok_or(Errno::EFAULT)?;
        let write_end = write_kaddr.checked_add(len).ok_or(Errno::EFAULT)?;
        if read_kaddr < write_end && write_kaddr < read_end {
            return Err(Errno::EFAULT);
        }

        // SAFETY: Both translated ranges are confined to one page, and the
        // MapManager lock keeps their backing MapArea/PageFrame objects alive.
        // The ranges were also checked to be non-overlapping, so creating a
        // shared source slice and a mutable destination slice is sound here.
        let read_slice = unsafe { core::slice::from_raw_parts(read_kaddr as *const u8, len) };
        // SAFETY: See the safety note above; the destination range is checked
        // to stay inside one translated page and is consumed before unlock.
        let write_slice = unsafe { core::slice::from_raw_parts_mut(write_kaddr as *mut u8, len) };
        Ok(f(read_slice, write_slice))
    }

    pub fn copy_to_user_buffer(&self, mut uaddr: usize, buffer: &[u8]) -> Result<(), Errno> {
        let mut left = buffer.len();
        let mut copied: usize = 0;

        while left > 0 {
            let page_offset = uaddr & (arch::PGSIZE - 1);
            let write_len = core::cmp::min(left, arch::PGSIZE - page_offset);

            self.with_translated_write(uaddr, write_len, |dst| {
                dst.copy_from_slice(&buffer[copied..copied + write_len]);
            })?;

            copied += write_len;
            left -= write_len;
            uaddr += write_len;
        }

        Ok(())
    }

    pub fn copy_to_user<T: Copy>(&self, uaddr: usize, value: T) -> Result<(), Errno> {
        // SAFETY: `value` is a properly initialized `T`, and the byte slice is
        // limited to exactly its in-memory representation for immediate copy.
        let buffer =
            unsafe { core::slice::from_raw_parts((&value as *const T) as *const u8, core::mem::size_of::<T>()) };
        self.copy_to_user_buffer(uaddr, buffer)
    }

    /// Copy a slice to user space
    pub fn copy_to_user_slice<T>(&self, uaddr: usize, slice: &[T]) -> SysResult<()> {
        // SAFETY: `slice` is an initialized slice of `T`; this only views its
        // existing contiguous storage as bytes for immediate copy.
        let buffer = unsafe { core::slice::from_raw_parts(slice.as_ptr() as *const u8, core::mem::size_of_val(slice)) };
        self.copy_to_user_buffer(uaddr, buffer)
    }

    pub fn copy_from_user_buffer(&self, mut uaddr: usize, buffer: &mut [u8]) -> Result<(), Errno> {
        let mut left = buffer.len();
        let mut copied: usize = 0;

        while left > 0 {
            let page_offset = uaddr & (arch::PGSIZE - 1);
            let read_len = core::cmp::min(left, arch::PGSIZE - page_offset);

            self.with_translated_read(uaddr, read_len, |src| {
                buffer[copied..copied + read_len].copy_from_slice(src);
            })?;

            copied += read_len;
            left -= read_len;
            uaddr += read_len;
        }

        Ok(())
    }

    pub fn copy_from_user<T: Copy>(&self, uaddr: usize) -> Result<T, Errno> {
        // SAFETY: This preserves the previous UserStruct contract: callers use
        // `T: Copy` types whose all-zero bit pattern is valid before overwrite.
        let mut value: T = unsafe { core::mem::zeroed() };
        // SAFETY: `value` is initialized storage for `T`; this byte view is
        // used only to fill that storage before returning the copied value.
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
        let mut result = tstr::<N>::new();

        loop {
            let page_offset = *uaddr & arch::PGMASK;
            let to_read = arch::PGSIZE - page_offset;
            let done = self.with_translated_read(*uaddr, to_read, |slice| {
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

                Ok(done)
            })??;

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
        let mut result = String::new();

        loop {
            let page_offset = *uaddr & arch::PGMASK;
            let to_read = arch::PGSIZE - page_offset;
            let done = self.with_translated_read(*uaddr, to_read, |slice| {
                let (bytes, done) = match slice.iter().position(|&b| b == 0) {
                    Some(pos) => (&slice[..pos], true),
                    None => (slice, false),
                };

                let part = core::str::from_utf8(bytes).map_err(|_| Errno::EINVAL)?;
                result.push_str(part);
                if !done && result.len() > max_size {
                    return Err(too_long_errno);
                }

                Ok(done)
            })??;

            if done {
                break;
            }

            *uaddr += to_read;
        }

        if result.len() > max_size {
            return Err(too_long_errno);
        }

        Ok(result)
    }

    pub fn copy_from_user_slice<T: Copy>(&self, uaddr: usize, slice: &mut [T]) -> SysResult<()> {
        // SAFETY: `slice` is valid mutable storage for `T`; this byte view is
        // used only to fill its existing contiguous storage from user memory.
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

    pub fn add_map_manager_watcher(&self, watcher: Arc<dyn MapManagerWatcher>) {
        self.map_manager.lock().add_watcher(watcher);
    }

    pub fn remove_map_manager_watcher(&self, watcher: &Arc<dyn MapManagerWatcher>) {
        self.map_manager.lock().remove_watcher(watcher);
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
        map_manager.cleanup(&self.pagetable);
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
