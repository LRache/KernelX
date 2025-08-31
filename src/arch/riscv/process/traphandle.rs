use crate::arch::riscv::csr::*;
use crate::arch::UserContext;
use crate::kdebug;
use crate::kernel::mm::MemAccessType;
use crate::kernel::scheduler::current;
use crate::kernel::trap;
use crate::kernel::syscall;
use crate::ktrace;
use crate::platform::config::TRAMPOLINE_BASE;
use crate::{platform, println};

unsafe extern "C" {
    fn asm_usertrap_entry (user_context: *mut   UserContext) -> !;
    fn asm_usertrap_return(user_context: *const UserContext) -> !;
}

fn handle_syscall() {
    let tcb = current::tcb();
    
    let user_entry = csrr_sepc() + 4;
    tcb.set_user_entry(user_entry);

    tcb.with_user_context_mut(|user_context|{
        let syscall_args: syscall::Args = [
            user_context.gpr[10], // a0
            user_context.gpr[11], // a1
            user_context.gpr[12], // a2
            user_context.gpr[13], // a3
            user_context.gpr[14], // a4
            user_context.gpr[15], // a5
            user_context.gpr[16], // a6
        ];

        let syscall_num = user_context.gpr[17]; // a7

        user_context.gpr[10] = trap::syscall(syscall_num, &syscall_args) as usize;

        // println!("Syscall handled: num={}, args={:?}, returning to user entry at {:#x}", 
        //      syscall_num, syscall_args, user_entry);
    });
}

pub fn usertrap_handler() -> ! {
    csrw_stvec(kerneltrap_handler as usize);

    // println!("Usertrap scause={:#x}, sepc={:#x}, stval={:#x}", csrr_scause(), csrr_sepc(), csrr_stval());

    // current::tcb().set_user_entry(csrr_sepc());
    
    match scause::cause() {
        scause::Cause::Trap(trap) => {
            match trap {
                scause::Trap::EcallU => handle_syscall(),
                scause::Trap::InstPageFault => {
                    let addr = csrr_stval();
                    trap::memory_fault(addr, MemAccessType::Execute);
                    current::tcb().set_user_entry(csrr_sepc());
                },
                scause::Trap::LoadPageFault => {
                    let addr = csrr_stval();
                    trap::memory_fault(addr, MemAccessType::Read);
                    current::tcb().set_user_entry(csrr_sepc());
                },
                scause::Trap::StorePageFault => {
                    let addr = csrr_stval();
                    trap::memory_fault(addr, MemAccessType::Write);
                    current::tcb().set_user_entry(csrr_sepc());
                },
                _ => {
                    // Handle other traps
                    println!("Usertrap scause={:#x}, sepc={:#x}, stval={:#x}", csrr_scause(), csrr_sepc(), csrr_stval());
                    platform::shutdown();
                }
            }
        },
        
        scause::Cause::Interrupt(interrupt) => {
            match interrupt {
                scause::Interrupt::Software => {
                    println!("Software interrupt occurred");
                },
                scause::Interrupt::Timer => {
                    println!("Timer interrupt occurred");
                    trap::timer_interrupt();
                },
                scause::Interrupt::External => {
                    println!("External interrupt occurred");
                },
                scause::Interrupt::Counter => {
                    println!("Counter interrupt occurred");
                },
            }
            println!("Interrupt occurred, returning to user mode");
        },
    }
    
    return_to_user();
}

fn usertrap_return(user_context: *const UserContext) -> ! {
    let trampoline_usertrap_return = 
        (TRAMPOLINE_BASE + (asm_usertrap_return as usize - asm_usertrap_entry as usize)) 
        as usize;
    
    unsafe {
        core::arch::asm!(
            "jr {target}",
            target = in(reg) trampoline_usertrap_return,
            in("a0") user_context,
            options(noreturn)
        );
    }
}

pub fn return_to_user() -> ! {
    let tcb = current::tcb();

    csrw_sepc(tcb.get_user_entry());
    csrw_stvec(TRAMPOLINE_BASE);
    csrw_sscratch(tcb.get_user_context_uaddr());

    let user_context_ptr = tcb.get_user_context_ptr();

    usertrap_return(user_context_ptr);
}

pub fn kerneltrap_handler() -> ! {
    println!("Kerneltrap scause={:#x}, sepc={:#x}, stval={:#x}",
             csrr_scause(), csrr_sepc(), csrr_stval());
    
    // Handle kernel traps here
    // For now, we just shutdown the platform
    platform::shutdown();
}
