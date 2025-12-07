// TODO: Test and verify the shared file mapping area implementation.
// TODO: Implement the swapped filed page frame

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::vec;
use spin::RwLock;

use crate::fs::InodeOps;
use crate::fs::inode::Index as InodeIndex;
use crate::kernel::mm::{MapPerm, PhysPageFrame, AddrSpace, MemAccessType};
use crate::kernel::mm::maparea::Area;
use crate::klib::SpinLock;
use crate::arch::{PageTable, PageTableTrait};
use crate::arch;

struct MappedFileEntry {
    inode: Arc<dyn InodeOps>,
    shared: BTreeMap<usize, PhysPageFrame>,
    ref_count: usize,
}

impl MappedFileEntry {
    fn get_page(&mut self, page_index: usize) -> Option<usize> {
        if let Some(frame) = self.shared.get(&page_index) {
            Some(frame.get_page())
        } else {
            // Load from inode
            let offset = page_index * arch::PGSIZE;
            let frame = PhysPageFrame::alloc_zeroed();
            self.inode.readat(frame.slice(), offset).expect("Failed to read.");
            let kpage = frame.get_page();
            self.shared.insert(page_index, frame);
            Some(kpage)
        }
    }
}

impl Drop for MappedFileEntry {
    fn drop(&mut self) {
        // Write back all pages
        for (page_index, frame) in self.shared.iter() {
            let offset = page_index * arch::PGSIZE;
            self.inode.writeat(frame.slice(), offset).expect("Failed to write back.");
        }
    }
}

struct Manager {
    mapped: SpinLock<BTreeMap<InodeIndex, Arc<SpinLock<MappedFileEntry>>>>,
}

impl Manager {
    pub const fn new() -> Self {
        Self {
            mapped: SpinLock::new(BTreeMap::new()),
        }
    }

    pub fn open_mapped_file(&self, inode: Arc<dyn InodeOps>, index: InodeIndex) -> Arc<SpinLock<MappedFileEntry>> {
        let mut mapped = self.mapped.lock();
        if let Some(entry) = mapped.get(&index) {
            entry.lock().ref_count += 1;
            return entry.clone();
        } else {
            let entry = Arc::new(SpinLock::new(MappedFileEntry {
                inode: inode,
                shared: BTreeMap::new(),
                ref_count: 1,
            }));
            mapped.insert(index, entry.clone());
            return entry;
        }
    }

    pub fn close_mapped_file(&self, index: InodeIndex) {
        let mut mapped = self.mapped.lock();
        let mut should_remove = false;
        if let Some(entry) = mapped.get(&index) {
            let mut entry_lock = entry.lock();
            entry_lock.ref_count -= 1;
            if entry_lock.ref_count == 0 {
                should_remove = true;
            }
        }
        if should_remove {
            mapped.remove(&index);
        }
    }
}

static MANAGER: Manager = Manager::new();

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameState {
    Unallocated,
    Allocated,
}

pub struct SharedFileMapArea {
    entry: Arc<SpinLock<MappedFileEntry>>,
    ubase: usize,
    offset: usize,
    states: Vec<FrameState>,
    perm: MapPerm,
    inode_index: InodeIndex,
}

impl SharedFileMapArea {
    pub fn new(
        ubase: usize,
        perm: MapPerm,
        inode: Arc<dyn InodeOps>,
        index: InodeIndex,
        offset: usize,
        page_count: usize
    ) -> Self {
        let states = vec![FrameState::Unallocated; page_count];
        let entry = MANAGER.open_mapped_file(inode, index);
        Self {
            entry,
            ubase,
            offset,
            states,
            perm,
            inode_index: index,
        }
    }

    fn translate(&self, uaddr: usize) -> Option<usize> {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index >= self.states.len() {
            return None;
        }

        let kpage = self.entry.lock().get_page(page_index)?;
        Some(kpage + uaddr & arch::PGSIZE)
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

    fn set_perm(&mut self, perm: MapPerm, pagetable: &RwLock<PageTable>) {
        self.perm = perm;
        let mut pagetable = pagetable.write();
        self.states.iter().enumerate().for_each(|(page_index, &state)| {
            if state == FrameState::Allocated {
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

    fn fork(&mut self, _self_pagetable: &RwLock<PageTable>, _fork_pagetable: &RwLock<PageTable>) -> Box<dyn Area> {
        let new_area = SharedFileMapArea {
            entry: self.entry.clone(),
            ubase: self.ubase,
            offset: self.offset,
            states: vec![FrameState::Unallocated; self.states.len()],
            perm: self.perm,
            inode_index: self.inode_index,
        };
        
        Box::new(new_area)
    }

    fn translate_read(&mut self, uaddr: usize, _addrspace: &Arc<AddrSpace>) -> Option<usize> {
        self.translate(uaddr)
    }

    fn translate_write(&mut self, uaddr: usize, _addrspace: &Arc<AddrSpace>) -> Option<usize> {
        self.translate(uaddr)
    }

    fn try_to_fix_memory_fault(
            &mut self, 
            uaddr: usize, 
            _access_type: MemAccessType, 
            addrspace: &Arc<AddrSpace>
        ) -> bool {
        let page_index = (uaddr - self.ubase) / arch::PGSIZE;
        if page_index >= self.states.len() {
            return false;
        }

        if self.states[page_index] == FrameState::Unallocated {
            let kpage = self.entry.lock().get_page(page_index).expect("Failed to get page in try_to_fix_memory_fault");
            let mut pagetable = addrspace.pagetable().write();
            pagetable.mmap(
                self.ubase + page_index * arch::PGSIZE,
                kpage,
                self.perm,
            );
            self.states[page_index] = FrameState::Allocated;
        }

        return true;
    }

    fn unmap(&mut self, pagetable: &RwLock<PageTable>) {
        let mut pagetable = pagetable.write();
        self.states.iter().enumerate().for_each(|(page_index, &state)| {
            if state == FrameState::Allocated {
                let uaddr = self.ubase + page_index * arch::PGSIZE;
                pagetable.munmap(uaddr);
            }
        });
        MANAGER.close_mapped_file(self.inode_index);
    }

    fn type_name(&self) -> &'static str {
        "SharedFileMapArea"
    }
}
