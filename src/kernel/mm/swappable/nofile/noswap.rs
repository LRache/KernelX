use alloc::sync::Arc;

use crate::kernel::mm::{AddrSpace, PhysPageFrame};

use super::super::swappable::{AccessDirty, SwapError, SwappableBackendOps, SwappableFrame, TlbInvalidationToken};

#[derive(Clone, Copy)]
pub struct AnonymousBackend;

pub type AnonymousSwappableFrame = SwappableFrame<AnonymousBackend>;

impl SwappableBackendOps for AnonymousBackend {
    type SwappedOutContext = ();

    fn inspect_access_dirty_no_flush(&self, frame: &PhysPageFrame) -> (Self::SwappedOutContext, AccessDirty, bool) {
        let _ = frame;
        ((), AccessDirty::default(), false)
    }

    fn unmap_no_flush(&self, context: &Self::SwappedOutContext, frame: &PhysPageFrame) -> Option<AccessDirty> {
        let _ = (context, frame);
        None
    }

    fn read_in(&self) -> Result<PhysPageFrame, SwapError> {
        Err(SwapError::InvalidBacking)
    }

    fn write_out(&mut self, frame: &PhysPageFrame, dirty: bool, _mapping_refs: usize) -> Result<(), SwapError> {
        let _ = (frame, dirty);
        Err(SwapError::InvalidBacking)
    }

    fn release(&mut self) {}

    fn is_swappable(&self) -> bool {
        false
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
