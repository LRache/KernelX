use alloc::sync::Arc;
use crate::kernel::mm::AddrSpace;

cfg_if::cfg_if! {
    if #[cfg(feature="swap-memory")] {
        use crate::kernel::mm::swappable;
        pub use swappable::SwappableNoFileFrame;
    } else {
        use crate::kernel::mm::PhysPageFrame;
        #[derive(Debug)]
        pub struct SwappableNoFileFrame {
            frame: PhysPageFrame,
            uaddr: usize,
        }

        impl SwappableNoFileFrame {
            pub fn alloc_zeroed(uaddr: usize, _addrspace: &AddrSpace) -> (Self, usize) {
                let frame = PhysPageFrame::alloc_zeroed();
                let kpage = frame.get_page();
                (Self { frame, uaddr}, kpage)
            }

            pub fn allocated(uaddr:usize, frame: PhysPageFrame, _addrspace: &AddrSpace) -> Self {
                Self { frame, uaddr }
            }
            
            pub fn uaddr(&self) -> usize {
                self.uaddr
            }

            pub fn copy(&self, _addrspace: &AddrSpace) -> (Self, usize) {
                let new_frame = self.frame.copy();
                let kpage = new_frame.get_page();
                (Self { frame: new_frame, uaddr: self.uaddr }, kpage)
            }

            pub fn get_page(&self) -> Option<usize> {
                Some(self.frame.get_page())
            }

            /// Get the kpage, if swapped out, swap in first
            pub fn get_page_swap_in(&self) -> usize {
                self.frame.get_page()
            }

            pub fn is_swapped_out(&self) -> bool {
                false
            }
        }
    }
}

pub enum FrameState {
    Unallocated,
    Allocated(Arc<SwappableNoFileFrame>),
    Cow(Arc<SwappableNoFileFrame>),
}

impl FrameState {
    pub fn is_unallocated(&self) -> bool {
        matches!(self, FrameState::Unallocated)
    }

    pub fn is_cow(&self) -> bool {
        matches!(self, FrameState::Cow(_))
    }

    pub fn cow_to_allocated(&mut self, addrspace: &AddrSpace) -> usize {
        let t = core::mem::replace(self, FrameState::Unallocated);
        match t {
            FrameState::Cow(frame) => {
                match Arc::try_unwrap(frame) {
                    Ok(f) => {
                        let kpage = f.get_page_swap_in();
                        *self = FrameState::Allocated(Arc::new(f));
                        kpage
                    }
                    Err(f) => {
                        let (new_frame, kpage) = f.copy(addrspace);
                        *self = FrameState::Allocated(Arc::new(new_frame));
                        kpage
                    }
                }
            }
            _ => unreachable!(),
        }
    }

    pub fn allocate(uaddr: usize, addrspace: &AddrSpace) -> (FrameState, usize) {
        let (frame, kpage) = SwappableNoFileFrame::alloc_zeroed(uaddr, addrspace);
        (FrameState::Allocated(Arc::new(frame)), kpage)
    }
}
