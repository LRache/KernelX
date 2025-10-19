use crate::arch::UserContextTrait;
use crate::kernel::mm::MemAccessType;
use crate::kernel::scheduler::current;
use crate::kernel::ipc::SignalNum;
use crate::kernel::syscall;
use crate::kernel::event::timer;
use crate::kwarn;

pub fn trap_return() {
    current::tcb().handle_signal();
}

pub fn timer_interrupt() {
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
    
    current::tcb().user_context().skip_syscall_instruction();

    current::schedule();

    ret
}

pub fn memory_fault(addr: usize, access_type: MemAccessType) {
    let fixed = current::addrspace().try_to_fix_memory_fault(addr, access_type);

    if !fixed {
        kwarn!("Failed to fix memory fault at address: {:#x}, access_type={:?}, pc={:#x}, tid={}, KILLED", addr, access_type, crate::arch::get_user_pc(), current::tid());
        current::pcb().send_signal(SignalNum::SIGSEGV as u32, current::tid(), None).unwrap();
        current::schedule();
    }
}
