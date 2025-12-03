use core::usize;
use alloc::sync::Arc;

use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::mm::swappable::AddrSpaceFamilyChain;
use crate::kernel::mm::swappable::swapper;
use crate::klib::SpinLock;

pub(super) struct AllocatedFrame {
    pub(super) frame: PhysPageFrame,
    pub(super) dirty: bool,
}

pub(super) enum State {
    Allocated(AllocatedFrame),
    SwappedOut,
}

pub(super) const NO_DISK_BLOCK: usize = usize::MAX;

pub(super) struct FrameState {
    pub(super) state: State,
    pub(super) disk_block: usize,
}

pub struct SwappableNoFileFrameInner {
    pub(super) state: SpinLock<FrameState>,
    pub(super) family_chain: AddrSpaceFamilyChain,
    pub uaddr: usize,
}

impl SwappableNoFileFrameInner {
    pub fn allocated(uaddr: usize, frame: PhysPageFrame, family_chain: AddrSpaceFamilyChain) -> Self {
        Self {
            state: SpinLock::new(
                FrameState { 
                    state: State::Allocated(AllocatedFrame { frame, dirty: false }), 
                    disk_block: NO_DISK_BLOCK 
                }
            ),
            family_chain,       
            uaddr,
        }
    }
}

pub struct SwappableNoFileFrame {
    inner: Arc<SwappableNoFileFrameInner>,
}

impl SwappableNoFileFrame {
    pub fn allocated(uaddr: usize, frame: PhysPageFrame, addrspace: &AddrSpace) -> Self {
        let inner = Arc::new(SwappableNoFileFrameInner::allocated(uaddr, frame, addrspace.family_chain().clone()));
        Self { inner }
    }

    pub fn alloc(uaddr: usize, addrspace: &AddrSpace) -> Self {
        let frame = PhysPageFrame::alloc();
        let kpage = frame.get_page();
        let frame = Self::allocated(uaddr, frame, addrspace);
        swapper::push_lru(kpage, frame.inner.clone());
        frame
    }

    pub fn alloc_zeroed(uaddr: usize, addrspace: &AddrSpace) -> (Self, usize) {
        let frame = PhysPageFrame::alloc_zeroed();
        let kpage = frame.get_page();
        let frame = Self::allocated(uaddr, frame, addrspace);
        swapper::push_lru(kpage, frame.inner.clone());
        (frame, kpage)
    }

    pub fn copy(&self, addrspace: &AddrSpace) -> (Self, usize) {
        let (inner, kpage) = self.inner.copy(addrspace);
        (SwappableNoFileFrame { inner }, kpage)
    }

    pub fn get_page(&self) -> Option<usize> {
        match &self.inner.state.lock().state {
            State::Allocated(allocated) => {
                Some(allocated.frame.get_page())
            },
            State::SwappedOut => {
                None
            }
        }
    }

    pub fn get_page_swap_in(&self) -> usize {
        self.inner.get_page_swap_in()
    }

    pub fn uaddr(&self) -> usize {
        self.inner.uaddr
    }

    pub fn is_swapped_out(&self) -> bool {
        matches!(&self.inner.state.lock().state, State::SwappedOut)
    }
}

impl Drop for SwappableNoFileFrame {
    fn drop(&mut self) {
        // When an SwappableNoFileFrame is dropped, it should be deallocated from the swapper.
        self.inner.free();
    }
}
