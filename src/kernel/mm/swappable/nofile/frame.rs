use alloc::sync::Arc;

use crate::kernel::mm::{AddrSpace, PhysPageFrame};

use super::super::swappable::{AccessDirty, SwapError, SwappableBackendOps, SwappableFrame, SwappableFramePin};
use super::family::{AnonMapFamily, AnonMapFamilyMember, AnonMapFamilyRegistration};
use super::swapspace::{SwapSlot, swap_space};

#[derive(Clone)]
pub struct AnonymousBackend {
    mapping: Arc<AnonMapFamily>,
    page_index: usize,
    swap_slot: Option<SwapSlot>,
}

impl AnonMapFamilyRegistration {
    pub fn page_backend(&self, page_index: usize) -> AnonymousBackend {
        AnonymousBackend {
            mapping: self.mapping(),
            page_index,
            swap_slot: None,
        }
    }
}

impl AnonymousBackend {
    fn collect_access_dirty(
        &self,
        members: &[AnonMapFamilyMember],
        frame: &PhysPageFrame,
        operation: impl Fn(&AddrSpace, usize, usize) -> Option<AccessDirty>,
    ) -> Option<AccessDirty> {
        let expected_kpage = frame.get_page();
        let mut result: Option<AccessDirty> = None;

        for member in members {
            let Some(uaddr) = member.page_uaddr(self.page_index) else {
                continue;
            };
            let Some(addrspace) = member.addrspace().upgrade() else {
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

/// The page frame finds the addrspace that maps it by each `backend.mapping.members`'s `addrspace` field,
/// and then checks if the page is mapped in that addrspace at the user virtual address corresponding to the page index.
/// If it is mapped, it checks if the page has been accessed or dirtied, and returns an `AccessDirty` struct indicating whether the page has been accessed or dirtied in any of the mappings.
pub type AnonymousSwappableFrame = SwappableFrame<AnonymousBackend>;
pub type AnonymousSwappableFramePin = SwappableFramePin<AnonymousBackend>;

impl SwappableBackendOps for AnonymousBackend {
    type SwappedOutContext = Arc<[AnonMapFamilyMember]>;

    fn is_swappable(&self) -> bool {
        swap_space().is_some()
    }

    fn inspect_access_dirty_no_flush(&self, frame: &PhysPageFrame) -> (Self::SwappedOutContext, AccessDirty, bool) {
        let snapshot = self.mapping.snapshot();
        let access_dirty = self.collect_access_dirty(&snapshot, frame, AddrSpace::take_access_dirty_if_maps_no_flush);
        let tlb_changed = access_dirty.is_some();
        (snapshot, access_dirty.unwrap_or_default(), tlb_changed)
    }

    fn unmap_no_flush(&self, snapshot: &Self::SwappedOutContext, frame: &PhysPageFrame) -> Option<AccessDirty> {
        self.collect_access_dirty(snapshot, frame, AddrSpace::unmap_if_maps_no_flush)
    }

    fn read_in(&self) -> Result<PhysPageFrame, SwapError> {
        let slot = self.swap_slot.expect("swapped-out anonymous page has no swap slot");
        let swap_space = swap_space().expect("anonymous swap space disappeared while a page was swapped out");
        let frame = PhysPageFrame::alloc_with_shrink_zeroed();
        swap_space.read_page(slot, &frame)?;
        Ok(frame)
    }

    fn write_out(&mut self, frame: &PhysPageFrame, dirty: bool, _mapping_refs: usize) -> Result<(), SwapError> {
        let swap_space = swap_space().expect("swappable anonymous page has no swap space");
        if let Some(slot) = self.swap_slot {
            if !dirty {
                return Ok(());
            }
            return swap_space.write_page(slot, frame);
        }

        let slot = swap_space.alloc_slot()?;
        match swap_space.write_page(slot, frame) {
            Ok(()) => {
                self.swap_slot = Some(slot);
                Ok(())
            }
            Err(error) => {
                swap_space.free_slot(slot);
                Err(error)
            }
        }
    }

    fn release(&mut self) {
        if let Some(slot) = self.swap_slot.take() {
            swap_space()
                .expect("anonymous swap space disappeared while a slot was allocated")
                .free_slot(slot);
        }
    }
}
