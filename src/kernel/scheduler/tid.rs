use core::sync::atomic::{AtomicI32, Ordering};

pub type Tid = i32;

pub const TID_START: Tid = 0;
pub const PID_MAX: Tid = 1 << 20;

static NEXT_TID: AtomicI32 = AtomicI32::new(TID_START);

pub fn alloc() -> Tid {
    NEXT_TID.fetch_add(1, Ordering::Relaxed)
}
