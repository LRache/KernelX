use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use fixedstr::tstr;
use spin::Lazy;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::kernel::config::{MAX_PATH_LEN, USER_RANDOM_ADDR_BASE};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::{Auxv, MapManagerWatcher, PinPageFrame, ReadChunk, WriteChunk};
use crate::kernel::mm::swappable::{SwappableBackendOps, SwappableFrameGuard};
use crate::kernel::mm::{PhysPageFrame, maparea};
use crate::klib::{SleepLock, SpinLock};

use super::swappable::AccessDirty;
use super::{MapPerm, MemAccessType, vdso};

static RANDOM_PAGE: Lazy<Arc<PhysPageFrame>> = Lazy::new(|| Arc::new(PhysPageFrame::alloc()));

fn create_pagetable() -> PageTable {
    let mut pagetable = PageTable::new_user();
    pagetable.mmap(USER_RANDOM_ADDR_BASE, &RANDOM_PAGE, MapPerm::R | MapPerm::U);

    vdso::map_to_pagetale(&mut pagetable);

    pagetable
}

pub struct AddrSpace {
    /// Number of TCBs still using this address space, excluding temporary `Arc`s.
    task_users: AtomicUsize,
    map_manager: SleepLock<maparea::Manager>,
    pagetable: SpinLock<PageTable>,
}

pub enum MmapPlacement {
    Hint(usize),
    Fixed(usize),
    FixedNoReplace(usize),
}

impl AddrSpace {
    pub fn new() -> Arc<Self> {
        Self::new_with_pagetable(create_pagetable())
    }

    pub fn new_with_pagetable(pagetable: PageTable) -> Arc<Self> {
        let addrspace = Arc::new(AddrSpace {
            task_users: AtomicUsize::new(0),
            map_manager: SleepLock::new(maparea::Manager::new(), "AddrSpace::map_manager"),
            pagetable: SpinLock::new(pagetable, "AddrSpace::pagetable"),
        });

        addrspace
    }

    pub(crate) fn acquire_task_user(&self) {
        let previous = self.task_users.fetch_add(1, Ordering::Relaxed);
        assert!(previous != usize::MAX, "AddrSpace::task_users overflow");
    }

    /// Releases one logical TCB user and cleans up the last user's mappings.
    ///
    /// This may sleep and must be called before the task becomes unschedulable.
    pub(crate) fn release_task_user(&self) {
        let previous = self.task_users.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "AddrSpace::task_users underflow");
        if previous == 1 {
            self.cleanup();
        }
    }

    pub fn fork(self: &Arc<Self>) -> Arc<AddrSpace> {
        let addrspace = Self::new_with_pagetable(create_pagetable());
        let new_map_manager = self.map_manager.lock().fork(&self.pagetable, &addrspace);
        *addrspace.map_manager.lock() = new_map_manager;

        addrspace
    }

    pub fn create_user_stack(self: &Arc<Self>, argv: &[&str], envp: &[&str], auxv: &Auxv) -> Result<usize, Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.create_user_stack(argv, envp, auxv, self)
    }

    pub fn map_area(&self, uaddr: usize, area: Box<dyn maparea::Area>) -> Result<(), Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.map_area(uaddr, area);

        Ok(())
    }

    pub fn mmap_area(self: &Arc<Self>, placement: MmapPlacement, mut area: Box<dyn maparea::Area>) -> SysResult<usize> {
        let mut map_manager = self.map_manager.lock();
        let page_count = area.page_count();
        let (ubase, replace) = match placement {
            MmapPlacement::Hint(addr) => {
                if addr == 0 || map_manager.is_map_range_overlapped(addr, page_count) {
                    (map_manager.find_mmap_ubase(page_count).ok_or(Errno::ENOMEM)?, false)
                } else {
                    (addr, false)
                }
            }
            MmapPlacement::Fixed(addr) => (addr, true),
            MmapPlacement::FixedNoReplace(addr) => {
                if map_manager.is_map_range_overlapped(addr, page_count) {
                    return Err(Errno::EEXIST);
                }
                (addr, false)
            }
        };

        area.set_ubase(ubase);
        area.bind_addrspace(self);
        if replace {
            map_manager.map_area_fixed(ubase, area, &self.pagetable);
        } else {
            map_manager.map_area(ubase, area);
        }

        Ok(ubase)
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

    pub fn increase_userbrk(self: &Arc<Self>, ubrk: usize) -> Result<usize, Errno> {
        let mut map_manager = self.map_manager.lock();
        map_manager.increase_userbrk(ubrk, &self.pagetable, self)
    }

    pub fn translate_write(&self, uaddr: usize, len: usize) -> SysResult<WriteChunk> {
        let offset = uaddr & arch::PGMASK;
        if len == 0 || len > arch::PGSIZE - offset {
            return Err(Errno::EFAULT);
        }
        let frame = self
            .map_manager
            .lock()
            .translate_write(uaddr, self)
            .ok_or(Errno::EFAULT)?;
        Ok(WriteChunk::new(frame, offset, len))
    }

    pub fn translate_read(&self, uaddr: usize, len: usize) -> SysResult<ReadChunk> {
        let offset = uaddr & arch::PGMASK;
        if len == 0 || len > arch::PGSIZE - offset {
            return Err(Errno::EFAULT);
        }
        let frame = self
            .map_manager
            .lock()
            .translate_read(uaddr, self)
            .ok_or(Errno::EFAULT)?;
        Ok(ReadChunk::new(frame, offset, len))
    }

    pub fn get_frame(self: &Arc<Self>, uaddr: usize) -> SysResult<PinPageFrame> {
        self.map_manager.lock().get_frame(uaddr, self).ok_or(Errno::EFAULT)
    }

    pub fn with_translated_read<F, R>(&self, uaddr: usize, len: usize, f: F) -> SysResult<R>
    where
        F: FnOnce(&[u8]) -> R,
    {
        if len == 0 {
            return Ok(f(&[]));
        }
        let chunk = self.translate_read(uaddr, len)?;
        Ok(f(&chunk))
    }

    pub fn with_translated_write<F, R>(&self, uaddr: usize, len: usize, f: F) -> SysResult<R>
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        if len == 0 {
            return Ok(f(&mut []));
        }
        let mut chunk = self.translate_write(uaddr, len)?;
        Ok(f(&mut chunk))
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

        let read_end = read_uaddr.checked_add(len).ok_or(Errno::EFAULT)?;
        let write_end = write_uaddr.checked_add(len).ok_or(Errno::EFAULT)?;
        if read_uaddr < write_end && write_uaddr < read_end {
            return Err(Errno::EFAULT);
        }

        let mut source = alloc::vec![0u8; len];
        self.with_translated_read(read_uaddr, len, |slice| source.copy_from_slice(slice))?;
        self.with_translated_write(write_uaddr, len, |destination| f(&source, destination))
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
    ) -> Result<(), maparea::MemoryFaultSignal> {
        let map_manager = &mut self.map_manager.lock();
        map_manager.try_to_fix_memory_fault(uaddr, access_type, self)
    }

    pub fn cleanup(&self) {
        // let pagetable = &mut self.pagetable.write();
        let mut map_manager = self.map_manager.lock();
        map_manager.cleanup(&self.pagetable);
    }

    pub fn take_access_dirty_if_maps_no_flush(&self, uaddr: usize, expected_kpage: usize) -> Option<AccessDirty> {
        self.pagetable
            .lock()
            .take_access_dirty_bit_with_check_no_flush(uaddr, expected_kpage)
            .map(AccessDirty::from)
    }

    pub fn unmap_if_maps_no_flush(&self, uaddr: usize, expected_kpage: usize) -> Option<AccessDirty> {
        self.pagetable
            .lock()
            .munmap_with_check_and_ad_no_flush(uaddr, expected_kpage)
            .map(AccessDirty::from)
    }

    pub fn mmap_swappable<B>(
        &self,
        uaddr: usize,
        guard: &crate::kernel::mm::swappable::SwappableFrameGuard<'_, B>,
        perm: MapPerm,
    ) where
        B: crate::kernel::mm::swappable::SwappableBackendOps,
    {
        // SAFETY: The guard keeps the logical page resident and its page lock
        // held until after the PTE is installed. The owning Area already
        // registered this address range before acquiring the guard.
        unsafe { self.pagetable.lock().mmap_raw(uaddr, guard.frame().get_page(), perm) };
    }

    pub fn mmap_replace_swappable<B>(&self, uaddr: usize, guard: &SwappableFrameGuard<'_, B>, perm: MapPerm)
    where
        B: SwappableBackendOps,
    {
        // SAFETY: The guard keeps the replacement frame resident for the
        // complete PTE update, and the Area retains the logical page owner.
        unsafe {
            self.pagetable
                .lock()
                .mmap_replace_raw(uaddr, guard.frame().get_page(), perm)
        };
    }

    pub fn mmap_replace_swappable_if_maps<B>(
        &self,
        uaddr: usize,
        expected: &crate::kernel::mm::swappable::SwappableFrameGuard<'_, B>,
        replacement: &crate::kernel::mm::swappable::SwappableFrameGuard<'_, B>,
        perm: MapPerm,
    ) -> Option<AccessDirty>
    where
        B: crate::kernel::mm::swappable::SwappableBackendOps,
    {
        let mut pagetable = self.pagetable.lock();
        let access_dirty = match pagetable.mmap_replace_with_check_and_ad(
            uaddr,
            expected.frame().get_page(),
            replacement.frame().get_page(),
            perm,
        ) {
            Some(access_dirty) => AccessDirty::from(access_dirty),
            None if pagetable.mapped_flag(uaddr).is_none() => {
                // SAFETY: Both page guards remain live while the same page-table
                // lock verifies that the slot is empty and installs the frame.
                unsafe { pagetable.mmap_raw(uaddr, replacement.frame().get_page(), perm) };
                return Some(AccessDirty::default());
            }
            None => return None,
        };
        Some(access_dirty)
    }
}
