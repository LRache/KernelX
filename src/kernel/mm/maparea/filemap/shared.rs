use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::arch;
use crate::arch::{PageTable, PageTableTrait};
use crate::fs::Inode;
use crate::fs::inode::Index as InodeIndex;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::{Area, MapAreaInfo, MemoryFaultSignal};
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType, PhysPageFrame};
use crate::kernel::uapi::FileSealFlags;
use crate::klib::SpinLock;

#[derive(Clone)]
enum FrameState {
    Unallocated,
    Loaded { frame: Arc<PhysPageFrame>, mapped: bool },
}

impl FrameState {
    fn is_mapped(&self) -> bool {
        matches!(self, FrameState::Loaded { mapped: true, .. })
    }
}

pub struct SharedFileMapArea {
    inode: Arc<Inode>,
    ubase: usize,
    offset: usize,
    states: Vec<FrameState>,
    perm: MapPerm,
    writable: bool,
    inode_index: InodeIndex,
    path: String,
    shared_mmap_accounted: bool,
    writable_accounted: bool,
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
        let writable_accounted = perm.contains(MapPerm::W);
        if let Some(seal_ops) = inode.as_seal_ops() {
            seal_ops.begin_shared_mmap(writable_accounted);
        }

        Self {
            inode,
            ubase,
            offset,
            states: vec![FrameState::Unallocated; page_count],
            perm,
            writable,
            inode_index: index,
            path,
            shared_mmap_accounted: true,
            writable_accounted,
        }
    }

    fn file_page_index(&self, page_index: usize) -> Option<usize> {
        (self.offset / arch::PGSIZE).checked_add(page_index)
    }

    fn ensure_page(&mut self, page_index: usize) -> SysResult<Option<usize>> {
        if page_index >= self.states.len() {
            return Ok(None);
        }

        if let FrameState::Loaded { frame, .. } = &self.states[page_index] {
            return Ok(Some(frame.get_page()));
        }

        let file_page_index = self.file_page_index(page_index).ok_or(Errno::EFBIG)?;
        let Some(frame) = self.inode.mmap_shared_page(file_page_index)? else {
            return Ok(None);
        };
        let kpage = frame.get_page();
        self.states[page_index] = FrameState::Loaded { frame, mapped: false };
        Ok(Some(kpage))
    }

    fn translate(&mut self, uaddr: usize) -> Option<usize> {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        let page_offset = (uaddr - self.ubase) % arch::PGSIZE;
        let kpage = self.ensure_page(page_index).ok()??;
        Some(kpage + page_offset)
    }

    fn end_shared_mmap_accounting(&mut self) {
        if self.shared_mmap_accounted {
            if let Some(seal_ops) = self.inode.as_seal_ops() {
                seal_ops.end_shared_mmap(self.writable_accounted);
            }
            self.shared_mmap_accounted = false;
            self.writable_accounted = false;
        }
    }
}

impl Area for SharedFileMapArea {
    fn ubase(&self) -> usize {
        self.ubase
    }

    fn set_ubase(&mut self, ubase: usize) {
        self.ubase = ubase;
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

    fn set_perm(&mut self, perm: MapPerm, pagetable: &SpinLock<PageTable>) {
        let new_writable_accounted = perm.contains(MapPerm::W);
        if self.shared_mmap_accounted && self.writable_accounted != new_writable_accounted {
            if let Some(seal_ops) = self.inode.as_seal_ops() {
                seal_ops.update_shared_mmap_writable(self.writable_accounted, new_writable_accounted);
            }
            self.writable_accounted = new_writable_accounted;
        }
        self.perm = perm;

        let mut pagetable = pagetable.lock();
        self.states.iter().enumerate().for_each(|(page_index, state)| {
            if state.is_mapped() {
                let uaddr = self.ubase + page_index * arch::PGSIZE;
                pagetable.mmap_replace_perm(uaddr, perm);
            }
        });
    }

    fn page_count(&self) -> usize {
        self.states.len()
    }

    fn size(&self) -> usize {
        self.states.len() * arch::PGSIZE
    }

    fn fork(&mut self, _self_pagetable: &SpinLock<PageTable>) -> Box<dyn Area> {
        let writable_accounted = self.perm.contains(MapPerm::W);
        if let Some(seal_ops) = self.inode.as_seal_ops() {
            seal_ops.begin_shared_mmap(writable_accounted);
        }
        let new_area = SharedFileMapArea {
            inode: self.inode.clone(),
            ubase: self.ubase,
            offset: self.offset,
            states: vec![FrameState::Unallocated; self.states.len()],
            perm: self.perm,
            writable: self.writable,
            inode_index: self.inode_index,
            path: self.path.clone(),
            shared_mmap_accounted: true,
            writable_accounted,
        };

        Box::new(new_area)
    }

    fn translate_read(&mut self, uaddr: usize, _addrspace: &AddrSpace) -> Option<usize> {
        self.translate(uaddr)
    }

    fn translate_write(&mut self, uaddr: usize, _addrspace: &AddrSpace) -> Option<usize> {
        self.translate(uaddr)
    }

    fn try_to_fix_memory_fault(
        &mut self,
        uaddr: usize,
        _access_type: MemAccessType,
        addrspace: &AddrSpace,
    ) -> Result<usize, MemoryFaultSignal> {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index >= self.states.len() {
            return Err(MemoryFaultSignal::Segv);
        }

        let Some(kpage) = self.ensure_page(page_index).map_err(|_| MemoryFaultSignal::Bus)? else {
            return Err(MemoryFaultSignal::Bus);
        };

        if !self.states[page_index].is_mapped() {
            let mut pagetable = addrspace.pagetable().lock();
            if let FrameState::Loaded { frame, .. } = &self.states[page_index] {
                pagetable.mmap(self.ubase + page_index * arch::PGSIZE, frame, self.perm);
            }
            if let FrameState::Loaded { mapped, .. } = &mut self.states[page_index] {
                *mapped = true;
            }
        }

        Ok(kpage + (uaddr - self.ubase) % arch::PGSIZE)
    }

    fn split(mut self: Box<Self>, uaddr: usize) -> (Box<dyn Area>, Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "Split address must be page-aligned");
        debug_assert!(uaddr > self.ubase, "Split address must be greater than ubase");
        debug_assert!(uaddr < self.ubase + self.size(), "Split address out of bounds");

        let split_index = (uaddr - self.ubase) / arch::PGSIZE;
        let split_offset = split_index * arch::PGSIZE;
        let right_states = self.states.split_off(split_index);

        let writable_accounted = self.perm.contains(MapPerm::W);
        if let Some(seal_ops) = self.inode.as_seal_ops() {
            seal_ops.begin_shared_mmap(writable_accounted);
        }
        let right = SharedFileMapArea {
            inode: self.inode.clone(),
            ubase: uaddr,
            offset: self.offset + split_offset,
            states: right_states,
            perm: self.perm,
            writable: self.writable,
            inode_index: self.inode_index,
            path: self.path.clone(),
            shared_mmap_accounted: true,
            writable_accounted,
        };

        (self, Box::new(right))
    }

    fn unmap(&mut self, pagetable: &SpinLock<PageTable>) {
        for page_index in 0..self.states.len() {
            let state = core::mem::replace(&mut self.states[page_index], FrameState::Unallocated);
            let FrameState::Loaded { frame, mapped } = state else {
                continue;
            };

            if mapped {
                let mut pagetable = pagetable.lock();
                let uaddr = self.ubase + page_index * arch::PGSIZE;
                // The page may not be mapped to the page table if it was loaded by
                // `translate_read` or `translate_write` but never accessed afterwards.
                let _ = pagetable.munmap(uaddr, &frame);
            }

            // Write back the page if it was loaded, regardless of whether it was mapped or not.
            // SAFETY: page_index MUST be valid, which is guaranteed by the loop condition.
            let file_page_index = self.file_page_index(page_index).unwrap();
            if let Err(e) = self.inode.writeback_mmap_shared_page(file_page_index, &frame) {
                crate::kwarn!("Failed to write back shared mmap page: {:?}", e);
            }
            drop(frame);

            // Release the page in the inode after writing back.
            if let Some(file_page_index) = self.file_page_index(page_index) {
                self.inode.release_mmap_shared_page(file_page_index);
            }
        }

        self.end_shared_mmap_accounting();
    }

    fn type_name(&self) -> &'static str {
        "SharedFileMapArea"
    }

    fn map_area_info(&self) -> MapAreaInfo {
        let mut info = MapAreaInfo::new(self.ubase(), self.ubase() + self.size(), self.perm);
        info.shared = true;
        info.offset = self.offset;
        info.dev_minor = self.inode_index.sno;
        info.inode = self.inode_index.ino as u64;
        info.path = Some(self.path.clone());
        info
    }
}

impl Drop for SharedFileMapArea {
    fn drop(&mut self) {
        self.end_shared_mmap_accounting();
    }
}
