use core::sync::atomic::{AtomicI32, Ordering};

use crate::kernel::syscall::UserStruct;

pub type Tid = i32;
impl UserStruct for Tid {}

pub const TID_START: Tid = 0;

static NEXT_TID: AtomicI32 = AtomicI32::new(TID_START);

pub fn alloc() -> Tid {
    NEXT_TID.fetch_add(1, Ordering::Relaxed)
}
