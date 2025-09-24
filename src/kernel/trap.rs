use crate::kernel::mm::MemAccessType;
use crate::kernel::scheduler::current;
use crate::kernel::syscall;
use crate::kernel::timer;
use crate::kinfo;
use crate::kwarn;

pub fn timer_interrupt() {
    // This function is called when a timer interrupt occurs.
    // It can be used to handle periodic tasks or scheduling.
    // For now, we will just print a message.
    kinfo!("Timer interrupt occurred.");

    timer::interrupt();

    if !current::is_clear() {
        current::schedule();
    }
}

pub fn syscall(num: usize, args: &syscall::Args) -> usize {
    let ret = match syscall::syscall(num, args) {
        Ok(ret) => ret,
        Err(errno) => {
            -(errno as isize) as usize
        }
    };

    ret
}

pub fn memory_fault(addr: usize, access_type: MemAccessType) {
    let fixed = current::addrspace().try_to_fix_memory_fault(addr, access_type);

    if !fixed {
        kwarn!("Failed to fix memory fault at address: {:#x}, pc={:#x}, KILLED", addr, crate::arch::get_user_pc());
        current::tcb().exit(255);
        current::schedule();
    }
}
