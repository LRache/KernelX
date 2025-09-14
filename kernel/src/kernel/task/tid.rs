use spin::Mutex;

use crate::kdebug;

pub type Tid = i32;

static NEXT_TID: Mutex<Tid> = Mutex::new(0);

pub fn alloc() -> Tid {
    let mut next_tid = NEXT_TID.lock();
    let tid = *next_tid;
    *next_tid += 1;
    kdebug!("alloc tid: {}", tid);
    tid
}
