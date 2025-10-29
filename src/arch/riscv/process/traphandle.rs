use crate::kernel::mm::MemAccessType;
use crate::kernel::scheduler::current;
use crate::kernel::trap;
use crate::kernel::syscall;
use crate::arch::riscv::csr::*;
use crate::arch::riscv::UserContext;
use crate::arch::UserContextTrait;
use crate::kinfo;
use crate::platform::config::TRAMPOLINE_BASE;

unsafe extern "C" {
    fn asm_usertrap_entry (user_context: *mut   UserContext) -> !;
    fn asm_usertrap_return(user_context: *const UserContext) -> !;
}

fn handle_syscall() {
    let tcb = current::tcb();

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
    });
}

unsafe extern "C" {
    fn asm_kerneltrap_entry() -> !;
}

pub fn usertrap_handler() -> ! {
    stvec::write(asm_kerneltrap_entry as usize);
    current::tcb().user_context().set_user_entry(sepc::read());

    // ktrace!("Usertrap scause={:#x}, sepc={:#x}, stval={:#x}",
    //          scause::read(), sepc::read(), stval::read());
    
    match scause::cause() {
        scause::Cause::Trap(trap) => {
            match trap {
                scause::Trap::EcallU => handle_syscall(),
                scause::Trap::InstPageFault => {
                    let addr = stval::read();
                    trap::memory_fault(addr, MemAccessType::Execute);
                },
                scause::Trap::LoadPageFault => {
                    let addr = stval::read();
                    trap::memory_fault(addr, MemAccessType::Read);
                },
                scause::Trap::StorePageFault => {
                    let addr = stval::read();
                    trap::memory_fault(addr, MemAccessType::Write);
                },
                scause::Trap::IllegalInst => {
                    trap::illegal_inst();
                }
                _ => {
                    panic!("Unhandled user trap: {:?}, sepc={:#x}, stval={:#x}, cause={:?}", trap, sepc::read(), stval::read(), scause::cause());
                }
            }
        },
        
        scause::Cause::Interrupt(interrupt) => {
            match interrupt {
                scause::Interrupt::Software => {
                    kinfo!("Software interrupt occurred");
                },
                scause::Interrupt::Timer => {
                    // kinfo!("Timer interrupt occurred");
                    trap::timer_interrupt();
                },
                scause::Interrupt::External => {
                    kinfo!("External interrupt occurred");
                },
                scause::Interrupt::Counter => {
                    kinfo!("Counter interrupt occurred");
                },
            }
            // println!("Interrupt occurred, returning to user mode");
        },
    }

    trap::trap_return();
    
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

    sepc::write(tcb.user_context().get_user_entry());
    stvec::write(TRAMPOLINE_BASE);
    sscratch::write(tcb.get_user_context_uaddr());

    Sstatus::read()
        .set_spie(true) // Enable interrupts in user mode
        .set_spp(true) // Set previous mode to user
        .write();

    let user_context_ptr = tcb.get_user_context_ptr();

    // ktrace!("Return to user mode: entry={:#x}, user_context={:#x}", tcb.user_context().get_user_entry(), user_context_ptr as usize);

    usertrap_return(user_context_ptr);
}

#[unsafe(no_mangle)]
pub fn kerneltrap_handler() {
    let sepc = sepc::read();

    match scause::cause() {
        scause::Cause::Trap(trap) => {
            match trap {
                _ => {
                    panic!("Unhandled kernel trap: {:?}, sepc={:#x}, stval={:#x}, cause={:?}", trap, sepc, stval::read(), scause::cause());
                }
            }
        },
        
        scause::Cause::Interrupt(interrupt) => {
            match interrupt {
                scause::Interrupt::Software => {
                    kinfo!("Kernel software interrupt occurred");
                },
                scause::Interrupt::Timer => {
                    // kinfo!("Kernel timer interrupt occurred");
                    trap::timer_interrupt();
                },
                scause::Interrupt::External => {
                    kinfo!("Kernel external interrupt occurred");
                },
                scause::Interrupt::Counter => {
                    kinfo!("Kernel counter interrupt occurred");
                },
            }
        },
        
    }

    Sstatus::read().set_spp(false).write(); // Set previous mode to supervisor

    sepc::write(sepc);
}
