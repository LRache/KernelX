use alloc::sync::Arc;
use core::ops::Deref;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::kernel::mm::PhysPageFrame;

use super::super::swappable::{
    AccessDirty, SwapError, SwappableFrame, SwappableFrameGuard, SwappableFramePin, TlbInvalidationToken,
};
use super::backend::FileBackend;

pub type FileSwappableFrame = SwappableFrame<FileBackend>;

pub struct FilePage {
    frame: Arc<FileSwappableFrame>,
    identity_pins: AtomicUsize,
}

impl FilePage {
    pub(super) fn new(frame: Arc<FileSwappableFrame>) -> Arc<Self> {
        Arc::new(Self {
            frame,
            identity_pins: AtomicUsize::new(0),
        })
    }

    pub(super) fn frame(&self) -> &Arc<FileSwappableFrame> {
        &self.frame
    }

    pub(super) fn identity_pins(&self) -> usize {
        self.identity_pins.load(Ordering::Acquire)
    }

    pub(super) fn pin_identity(self: &Arc<Self>) -> FilePageIdentityPin {
        self.identity_pins
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |pins| pins.checked_add(1))
            .expect("file page identity pin count overflow");
        FilePageIdentityPin { page: self.clone() }
    }

    pub fn ensure_page(&self) -> Result<SwappableFrameGuard<'_, FileBackend>, SwapError> {
        self.frame.ensure_page()
    }

    pub fn try_get_page(&self) -> Option<SwappableFrameGuard<'_, FileBackend>> {
        self.frame.try_get_page()
    }

    pub(super) fn ensure_page_with<E>(
        &self,
        read_in: impl FnOnce(&FileBackend) -> Result<Option<PhysPageFrame>, E>,
    ) -> Result<Option<SwappableFrameGuard<'_, FileBackend>>, E> {
        self.frame.ensure_page_with(read_in)
    }

    pub fn pin_page(&self, write: bool) -> Result<SwappableFramePin<FileBackend>, SwapError> {
        self.frame.pin_page(write)
    }

    pub fn collect_mapped_access_dirty(&self) -> bool {
        self.frame.collect_mapped_access_dirty()
    }

    pub fn collect_mapped_access_dirty_no_flush(&self) -> (bool, usize) {
        self.frame.collect_mapped_access_dirty_no_flush()
    }

    pub fn clear_dirty(&self) {
        self.frame.clear_dirty();
    }

    pub fn is_invalid(&self) -> bool {
        self.frame.is_invalid()
    }

    pub fn invalidate(&self) {
        self.frame.invalidate();
    }

    pub fn with_resident_and_record_ad<R>(
        &self,
        accessed: bool,
        f: impl FnOnce(&PhysPageFrame) -> (R, AccessDirty),
    ) -> Option<R> {
        self.frame.with_resident_and_record_ad(accessed, f)
    }

    pub fn begin_tlb_invalidation(
        &self,
        token: &TlbInvalidationToken,
        unmap: impl FnOnce(&PhysPageFrame) -> Option<AccessDirty>,
    ) {
        self.frame.begin_tlb_invalidation(token, unmap);
    }

    pub fn finish_tlb_invalidation(&self, token: &TlbInvalidationToken) -> bool {
        self.frame.finish_tlb_invalidation(token)
    }
}

pub struct FilePageIdentityPin {
    page: Arc<FilePage>,
}

impl Deref for FilePageIdentityPin {
    type Target = FilePage;

    fn deref(&self) -> &Self::Target {
        &self.page
    }
}

impl Drop for FilePageIdentityPin {
    fn drop(&mut self) {
        let identity_pins = self
            .page
            .identity_pins
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pins| pins.checked_sub(1))
            .expect("file page identity pin count underflow");
        if identity_pins == 1 {
            self.page.frame.request_remove_unmapped_page();
        }
    }
}

pub struct FilePageMappingPin {
    page: Arc<FilePage>,
}

impl FilePageMappingPin {
    pub(super) fn new(page: Arc<FilePage>) -> Self {
        page.frame.add_mapping_ref();
        Self { page }
    }
}

impl Deref for FilePageMappingPin {
    type Target = FilePage;

    fn deref(&self) -> &Self::Target {
        &self.page
    }
}

impl Drop for FilePageMappingPin {
    fn drop(&mut self) {
        self.page.frame.release_mapping_ref();
    }
}

pub enum SharedFilePage {
    Stable(Arc<PhysPageFrame>),
    Swappable(FilePageMappingPin),
}
