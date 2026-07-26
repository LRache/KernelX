use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};

use crate::fs::Inode;
use crate::kernel::errno::SysResult;
use crate::kernel::mm::PhysPageFrame;
use crate::klib::SleepLock;

use super::super::swappable::SwappableFramePin;
use super::backend::{FileBackend, FileBackendState};
use super::frame::{FilePage, FilePageIdentityPin, FilePageMappingPin, FileSwappableFrame};
use super::rmap::{FileMapRegistration, FileRmap};

/// A mapping of a file's pages to physical frames,
/// also as a cache of the inode's pages in memory.
pub struct FileMapping {
    /// The backend state for the file mapping, containing the inode.
    ///
    /// Shared by all the shared-file-map **PAGE**s that mapped this file.
    backend: Arc<FileBackendState>,

    /// The mapping of file page indices to the corresponding swappable frames.
    pages: SleepLock<BTreeMap<usize, Arc<FilePage>>>,
}

impl FileMapping {
    pub fn new(inode: Weak<Inode>) -> Arc<Self> {
        Arc::new_cyclic(|mapping| {
            let rmap = Arc::new(FileRmap::new());
            Self {
                backend: FileBackendState::new(inode, mapping.clone(), rmap),
                pages: SleepLock::new(BTreeMap::new(), "FileMapping::pages"),
            }
        })
    }

    pub fn registration(&self) -> FileMapRegistration {
        FileMapRegistration::new(self.backend.rmap.clone())
    }

    /// Acquire a page for a shared file mapping, loading it if necessary.
    /// The returned pin owns one mapping reference until it is dropped.
    pub fn acquire_mapped_page(
        &self,
        file_page_index: usize,
        load: impl FnOnce() -> SysResult<Option<PhysPageFrame>>,
    ) -> SysResult<Option<FilePageMappingPin>> {
        let page = {
            let mut pages = self.pages.lock();
            let page = if let Some(page) = pages.get(&file_page_index) {
                page.clone()
            } else {
                let page = FilePage::new(FileSwappableFrame::new_out(
                    FileBackend::new(self.backend.clone(), file_page_index),
                    0,
                ));
                pages.insert(file_page_index, page.clone());
                page
            };
            FilePageMappingPin::new(page)
        };

        match page.ensure_page_with(|_| load()) {
            Ok(Some(_guard)) => {}
            Ok(None) => {
                self.remove_invalid_page(file_page_index, &page);
                return Ok(None);
            }
            Err(err) => return Err(err),
        };

        Ok(Some(page))
    }

    /// Acquire a page for a cache operation, loading it if necessary.
    /// The returned pin keeps this page as the canonical cache identity, but
    /// does not prevent its physical frame from being swapped out.
    pub fn acquire_cached_page(
        &self,
        file_page_index: usize,
        load: impl FnOnce() -> SysResult<Option<PhysPageFrame>>,
    ) -> SysResult<Option<FilePageIdentityPin>> {
        let page = self.acquire_cached_identity(file_page_index);

        match page.ensure_page_with(|_| load()) {
            Ok(Some(_guard)) => {}
            Ok(None) => {
                self.remove_invalid_page(file_page_index, &page);
                return Ok(None);
            }
            Err(err) => return Err(err),
        }
        Ok(Some(page))
    }

    /// Acquire and resident-pin a page for a cache operation.
    ///
    /// The resident pin is taken before the page's inner lock is released, so
    /// reclaim cannot evict a newly loaded page between acquisition and pinning.
    pub fn acquire_cached_page_pinned(
        &self,
        file_page_index: usize,
        load: impl FnOnce() -> SysResult<Option<PhysPageFrame>>,
    ) -> SysResult<Option<(FilePageIdentityPin, SwappableFramePin<FileBackend>)>> {
        let page = self.acquire_cached_identity(file_page_index);

        let resident_pin = match page.ensure_page_with(|_| load()) {
            Ok(Some(mut guard)) => guard.pin_page(false),
            Ok(None) => {
                self.remove_invalid_page(file_page_index, &page);
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        Ok(Some((page, resident_pin)))
    }

    fn acquire_cached_identity(&self, file_page_index: usize) -> FilePageIdentityPin {
        let mut pages = self.pages.lock();
        let page = if let Some(page) = pages.get(&file_page_index) {
            page.clone()
        } else {
            let page = FilePage::new(FileSwappableFrame::new_out(
                FileBackend::new(self.backend.clone(), file_page_index),
                0,
            ));
            pages.insert(file_page_index, page.clone());
            page
        };
        page.pin_identity()
    }

    pub fn cached_page(&self, file_page_index: usize) -> Option<FilePageIdentityPin> {
        let page = {
            let pages = self.pages.lock();
            pages.get(&file_page_index)?.pin_identity()
        };
        if page.is_invalid() {
            self.remove_invalid_page(file_page_index, &page);
            return None;
        }
        Some(page)
    }

    pub(super) fn remove_page(&self, file_page_index: usize, expected: &Arc<FileSwappableFrame>) {
        let mut pages = self.pages.lock();
        let Some(page) = pages.get(&file_page_index) else {
            return;
        };
        if Arc::ptr_eq(page.frame(), expected) && page.identity_pins() == 0 && expected.can_remove_unmapped_out() {
            pages.remove(&file_page_index);
        }
    }

    fn remove_invalid_page(&self, file_page_index: usize, expected: &FilePage) {
        let mut pages = self.pages.lock();
        let Some(page) = pages.get(&file_page_index) else {
            return;
        };
        if Arc::ptr_eq(page.frame(), expected.frame()) {
            pages.remove(&file_page_index);
        }
    }

    pub fn pages_snapshot(&self) -> alloc::vec::Vec<(usize, FilePageIdentityPin)> {
        self.pages
            .lock()
            .iter()
            .map(|(&index, page)| (index, page.pin_identity()))
            .collect()
    }

    pub fn invalidate_after(&self, page_count: usize) {
        let pages = {
            let mut pages = self.pages.lock();
            pages
                .split_off(&page_count)
                .into_values()
                .collect::<alloc::vec::Vec<_>>()
        };
        for page in pages {
            page.invalidate();
        }
    }
}
