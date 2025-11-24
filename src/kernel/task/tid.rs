use spin::Mutex;

use crate::kernel::syscall::UserStruct;

pub type Tid = i32;
pub type Pid = Tid;

impl UserStruct for Tid {}

static NEXT_TID: Mutex<Tid> = Mutex::new(0);

pub fn alloc() -> Tid {
    let mut next_tid = NEXT_TID.lock();
    let tid = *next_tid;
    *next_tid += 1;
    tid
}
