use alloc::sync::Arc;

pub use crate::kernel::mm::swappable::{AnonMapFamilyRegistration, AnonymousSwappableFrame as SwappablePageFrame};

pub enum FrameState {
    Unallocated,
    Allocated(Arc<SwappablePageFrame>),
    Cow(Arc<SwappablePageFrame>),
}

impl FrameState {
    pub fn is_unallocated(&self) -> bool {
        matches!(self, FrameState::Unallocated)
    }

    pub fn is_cow(&self) -> bool {
        matches!(self, FrameState::Cow(_))
    }
}
