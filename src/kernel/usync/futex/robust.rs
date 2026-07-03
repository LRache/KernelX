use crate::kernel::syscall::UserStruct;
use crate::kernel::task::TCB;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RobustList {
    next: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, UserStruct)]
pub struct RobustListHead {
    list: RobustList,
    futex_offset: usize,
    pending: usize,
}

impl TCB {
    pub fn set_robust_list(&self, uaddr: usize) {
        *self.robust_list.lock() = Some(uaddr);
    }

    pub fn get_robust_list(&self) -> Option<usize> {
        *self.robust_list.lock()
    }
}
