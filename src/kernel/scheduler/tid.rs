use spin::Mutex;

use crate::kernel::syscall::UserStruct;

pub type Tid = i32;

impl UserStruct for Tid {}

pub const TID_START: Tid = 1;

static NEXT_TID: Mutex<Tid> = Mutex::new(TID_START);

pub fn alloc() -> Tid {
    let mut next_tid = NEXT_TID.lock();
    let tid = *next_tid;
    *next_tid += 1;
    tid
}
