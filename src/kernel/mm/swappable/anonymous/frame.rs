use core::usize;
use alloc::sync::Arc;

use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::mm::swappable::AddrSpaceFamilyChain;
use crate::klib::SpinLock;

use super::swapper;

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

pub struct AnonymousFrameInner {
    pub(super) state: SpinLock<FrameState>,
    pub(super) family_chain: AddrSpaceFamilyChain,
    pub uaddr: usize,
}

impl AnonymousFrameInner {
    pub fn allocated(frame: PhysPageFrame, family_chain: AddrSpaceFamilyChain, uaddr: usize) -> Self {
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

    pub fn alloc(uaddr: usize, addrspace: &AddrSpace) -> (Self, usize) {
        let frame = PhysPageFrame::alloc_with_shrink_zeroed();
        let kpage = frame.get_page();
        let frame = Self::allocated(uaddr, frame, addrspace.family_chain().clone());
        swapper::push_lru(kpage, frame.inner.clone());
        (frame, kpage)
    }

    pub fn copy(&self, addrspace: &AddrSpace) -> (AnonymousFrame, usize) {
        let state = self.inner.state.lock();
        let new_frame = match &state.state {
            State::Allocated(allocated) => {
                allocated.frame.copy()
            }
            State::SwappedOut => {
                let new_frame = PhysPageFrame::alloc_with_shrink_zeroed();
                debug_assert!(state.disk_block != NO_DISK_BLOCK);
                swapper::read_swapped_page(state.disk_block, &new_frame);
                new_frame
            }
        };
        let kpage = new_frame.get_page();
        let allocated = AnonymousFrame::allocated(self.inner.uaddr, new_frame, addrspace.family_chain().clone());
        swapper::push_lru(kpage, allocated.inner.clone());
        (allocated, kpage)
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
        let mut state = self.inner.state.lock();
        match &state.state {
            State::Allocated(allocated) => {
                allocated.frame.get_page()
            },
            State::SwappedOut => {
                let frame = swapper::swap_in_page(state.disk_block, &self.inner);
                let page = frame.get_page();
                state.state = State::Allocated(AllocatedFrame { frame, dirty: false });

                page
            }
        }
    }

    pub fn uaddr(&self) -> usize {
        self.inner.uaddr
    }

    pub fn is_swapped_out(&self) -> bool {
        matches!(&self.inner.state.lock().state, State::SwappedOut)
    }
}

impl Drop for AnonymousFrame {
    fn drop(&mut self) {
        // When an AnonymousFrame is dropped, it should be deallocated from the swapper.
        swapper::dealloc(&self.inner);
    }
}
