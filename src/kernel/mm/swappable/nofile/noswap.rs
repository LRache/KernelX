use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::kernel::mm::{AddrSpace, PhysPageFrame};

use super::super::swappable::{AccessDirty, ResidentPageGuard, SwapError, TlbInvalidationToken};

#[derive(Clone, Copy)]
pub struct AnonymousBackend;

pub struct AnonymousSwappableFrame {
    frame: PhysPageFrame,
    mapping_refs: AtomicUsize,
}

impl AnonymousSwappableFrame {
    pub fn new_mapped(_backend: AnonymousBackend, frame: PhysPageFrame) -> Arc<Self> {
        Arc::new(Self {
            frame,
            mapping_refs: AtomicUsize::new(1),
        })
    }

    pub fn ensure_page(self: &Arc<Self>) -> Result<AnonymousSwappableFrameGuard<'_>, SwapError> {
        Ok(AnonymousSwappableFrameGuard { owner: self })
    }

    pub fn pin_page(self: &Arc<Self>, _write: bool) -> Result<AnonymousSwappableFramePin, SwapError> {
        Ok(AnonymousSwappableFramePin { owner: self.clone() })
    }

    pub fn mapping_refs(&self) -> usize {
        self.mapping_refs.load(Ordering::Acquire)
    }

    pub fn add_mapping_ref(&self) {
        self.mapping_refs
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |refs| refs.checked_add(1))
            .expect("anonymous mapping reference count overflow");
    }

    pub fn release_mapping_ref(&self) -> usize {
        let previous = self
            .mapping_refs
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |refs| refs.checked_sub(1))
            .expect("anonymous mapping reference count underflow");
        previous - 1
    }

    pub fn with_resident_and_record_ad<R>(
        &self,
        _accessed: bool,
        f: impl FnOnce(&PhysPageFrame) -> (R, AccessDirty),
    ) -> Option<R> {
        let (result, _) = f(&self.frame);
        Some(result)
    }

    pub fn begin_tlb_invalidation(
        &self,
        _token: &TlbInvalidationToken,
        unmap: impl FnOnce(&PhysPageFrame) -> Option<AccessDirty>,
    ) {
        let _ = unmap(&self.frame);
    }

    pub fn finish_tlb_invalidation(&self, _token: &TlbInvalidationToken) -> bool {
        true
    }
}

pub struct AnonymousSwappableFrameGuard<'a> {
    owner: &'a AnonymousSwappableFrame,
}

impl AnonymousSwappableFrameGuard<'_> {
    pub fn frame(&self) -> &PhysPageFrame {
        &self.owner.frame
    }

    pub fn mark_dirty(&mut self) {}

    pub fn release_mapping_ref(&mut self) -> usize {
        self.owner.release_mapping_ref()
    }
}

impl ResidentPageGuard for AnonymousSwappableFrameGuard<'_> {
    fn frame(&self) -> &PhysPageFrame {
        AnonymousSwappableFrameGuard::frame(self)
    }
}

pub struct AnonymousSwappableFramePin {
    owner: Arc<AnonymousSwappableFrame>,
}

impl AnonymousSwappableFramePin {
    pub fn kpage(&self) -> usize {
        self.owner.frame.get_page()
    }
}

pub struct AnonMapFamilyRegistration;

impl AnonMapFamilyRegistration {
    pub fn new_root() -> Self {
        Self
    }

    pub fn page_backend(&self, page_index: usize) -> AnonymousBackend {
        let _ = page_index;
        AnonymousBackend
    }

    pub fn is_bound(&self) -> bool {
        true
    }

    pub fn with_tlb_invalidation_batch<R>(&self, f: impl FnOnce(&TlbInvalidationToken) -> R) -> R {
        // Without swap, Area/object ownership keeps every physical frame alive
        // until its PTEs are cleared and the TLB invalidation is complete.
        f(&TlbInvalidationToken)
    }

    pub fn unbound_sibling(&self) -> Self {
        Self
    }

    pub fn fork(&self, addrspace: &Arc<AddrSpace>, ubase: usize, page_base: usize, page_count: usize) -> Self {
        let _ = (addrspace, ubase, page_base, page_count);
        Self
    }

    pub fn bind(&mut self, addrspace: &Arc<AddrSpace>, ubase: usize, page_base: usize, page_count: usize) {
        let _ = (addrspace, ubase, page_base, page_count);
    }

    pub fn update(&self, ubase: usize, page_base: usize, page_count: usize) {
        let _ = (ubase, page_base, page_count);
    }

    pub fn split(
        &mut self,
        left_ubase: usize,
        left_page_base: usize,
        left_page_count: usize,
        right_ubase: usize,
        right_page_base: usize,
        right_page_count: usize,
    ) -> Self {
        let _ = (
            left_ubase,
            left_page_base,
            left_page_count,
            right_ubase,
            right_page_base,
            right_page_count,
        );
        Self
    }

    pub fn unregister(&mut self) {}
}
