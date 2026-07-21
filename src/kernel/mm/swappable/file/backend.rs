use alloc::sync::{Arc, Weak};

use crate::fs::Inode;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};

use super::super::swappable::{AccessDirty, SwapError, SwappableBackendOps};
use super::frame::FileSwappableFrame;
use super::mapping::FileMapping;
use super::rmap::{FileMapEntry, FileRmap};

pub(super) struct FileBackendState {
    inode: Weak<Inode>,
    mapping: Weak<FileMapping>,
    pub(super) rmap: Arc<FileRmap>,
}

impl FileBackendState {
    pub(super) fn new(inode: Weak<Inode>, mapping: Weak<FileMapping>, rmap: Arc<FileRmap>) -> Arc<Self> {
        Arc::new(Self { inode, mapping, rmap })
    }
}

#[derive(Clone)]
pub struct FileBackend {
    state: Arc<FileBackendState>,
    page_index: usize,
}

impl FileBackend {
    pub(super) fn new(state: Arc<FileBackendState>, page_index: usize) -> Self {
        Self { state, page_index }
    }

    fn collect_access_dirty(
        &self,
        entries: &[FileMapEntry],
        frame: &PhysPageFrame,
        operation: impl Fn(&AddrSpace, usize, usize) -> Option<AccessDirty>,
    ) -> Option<AccessDirty> {
        let expected_kpage = frame.get_page();
        let mut result: Option<AccessDirty> = None;

        for entry in entries {
            let Some(uaddr) = entry.page_uaddr(self.page_index) else {
                continue;
            };
            let Some(addrspace) = entry.addrspace().upgrade() else {
                continue;
            };
            if let Some(access_dirty) = operation(&addrspace, uaddr, expected_kpage) {
                let result = result.get_or_insert_default();
                result.accessed |= access_dirty.accessed;
                result.dirty |= access_dirty.dirty;
            }
        }

        result
    }
}

impl SwappableBackendOps for FileBackend {
    type SwappedOutContext = Arc<[FileMapEntry]>;

    fn inspect_access_dirty_no_flush(&self, frame: &PhysPageFrame) -> (Self::SwappedOutContext, AccessDirty, bool) {
        let snapshot = self.state.rmap.snapshot();
        let access_dirty = self.collect_access_dirty(&snapshot, frame, AddrSpace::take_access_dirty_if_maps_no_flush);
        let tlb_changed = access_dirty.is_some();
        (snapshot, access_dirty.unwrap_or_default(), tlb_changed)
    }

    fn unmap_no_flush(&self, snapshot: &Self::SwappedOutContext, frame: &PhysPageFrame) -> Option<AccessDirty> {
        self.collect_access_dirty(snapshot, frame, AddrSpace::unmap_if_maps_no_flush)
    }

    fn read_in(&self) -> Result<PhysPageFrame, SwapError> {
        let inode = self.state.inode.upgrade().ok_or(SwapError::InvalidBacking)?;
        inode
            .load_raw_page(self.page_index)
            .map_err(|_| SwapError::Io)?
            .ok_or(SwapError::InvalidBacking)
    }

    fn write_out(&mut self, frame: &PhysPageFrame, dirty: bool, mapping_refs: usize) -> Result<(), SwapError> {
        // Only write back the page if it is dirty.
        // Don't remove the page from the mapping if it is still mapped in any address space.
        // If we remove the page from the mapping, when the second addrspace try to get the page,
        // it will get a new page different from the first one, which will cause data inconsistency.
        let inode = self.state.inode.upgrade().ok_or(SwapError::InvalidBacking)?;
        if dirty {
            inode
                .writeback_mmap_shared_page(self.page_index, frame)
                .map_err(|_| SwapError::Io)?;
        }

        // Don't call `remove_unmapped_page` even if the `mapping_refs = 0`
        let _ = mapping_refs;
        Ok(())
    }

    fn release(&mut self) {}

    fn retain_unmapped_pages(&self) -> bool {
        // The file page backend uses the swappable LRU as the cache of the inode's pages,
        // so it should retain unmapped pages in memory.
        true
    }

    fn remove_unmapped_page(&self, page: &Arc<FileSwappableFrame>) {
        // Synchronize the page removal with the mapping
        // so that the mapping backend can remove the page from its list of pages.
        if let Some(mapping) = self.state.mapping.upgrade() {
            mapping.remove_page(self.page_index, page);
        }
    }
}
