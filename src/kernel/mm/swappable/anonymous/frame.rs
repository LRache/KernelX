use alloc::sync::Arc;

use crate::kernel::mm::PhysPageFrame;
use crate::kernel::mm::swappable::AddrSpaceFamilyChain;
use crate::klib::SpinLock;

use super::swapper;

pub enum AnonymousFrameStatus {
    Allocated(PhysPageFrame),
    SwappedOut(usize), /* `diskpos: usize` The start block that stored the page  */
}

pub struct AnonymousFrameInner {
    pub(super) status: SpinLock<AnonymousFrameStatus>,
    pub(super) family_chain: AddrSpaceFamilyChain,
    pub uaddr: usize,
}

impl AnonymousFrameInner {
    pub fn allocated(frame: PhysPageFrame, family_chain: AddrSpaceFamilyChain, uaddr: usize) -> Self {
        Self {
            status: SpinLock::new(AnonymousFrameStatus::Allocated(frame)),
            family_chain,
            uaddr,
        }
    }
}

pub struct AnonymousFrame {
    inner: Arc<AnonymousFrameInner>,
}

impl AnonymousFrame {
    pub fn new(inner: Arc<AnonymousFrameInner>) -> Self {
        Self { inner }
    }

    pub fn allocated(uaddr: usize, frame: PhysPageFrame, family_chain: AddrSpaceFamilyChain) -> Self {
        let inner = Arc::new(AnonymousFrameInner::allocated(frame, family_chain, uaddr));
        Self::new(inner)
    }

    pub fn alloc(uaddr: usize, family_chain: AddrSpaceFamilyChain) -> Self {
        let frame = swapper::alloc(family_chain, uaddr);
        frame
    }

    pub fn copy(&self, new_family_chain: AddrSpaceFamilyChain) -> AnonymousFrame {
        match &*self.inner.status.lock() {
            AnonymousFrameStatus::Allocated(allocated) => {
                let new_frame = allocated.copy();
                AnonymousFrame::allocated(self.inner.uaddr, new_frame, new_family_chain)
            }
            AnonymousFrameStatus::SwappedOut(diskpos) => {
                let new_frame = PhysPageFrame::alloc_with_shrink_zeroed();
                swapper::read_swapped_page(*diskpos, &new_frame);
                AnonymousFrame::allocated(self.inner.uaddr, new_frame, new_family_chain)
            }
        }
    }

    pub fn page(&self) -> Option<usize> {
        match &*self.inner.status.lock() {
            AnonymousFrameStatus::Allocated(allocated) => {
                Some(allocated.get_page())
            },
            AnonymousFrameStatus::SwappedOut(_) => {
                None
            }
        }
    }

    pub fn swap_in(&self) -> usize {
        swapper::swap_in_page(&self.inner)
    }

    pub fn uaddr(&self) -> usize {
        self.inner.uaddr
    }

    pub fn is_swapped_out(&self) -> bool {
        matches!(&*self.inner.status.lock(), AnonymousFrameStatus::SwappedOut(_))
    }
}

impl Drop for AnonymousFrame {
    fn drop(&mut self) {
        // When an AnonymousFrame is dropped, it should be deallocated from the swapper.
        swapper::dealloc(&self.inner);
    }
}
