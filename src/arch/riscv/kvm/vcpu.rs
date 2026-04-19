use crate::arch::riscv::csr::scause::{Cause, Trap};
use crate::arch::riscv::csr::{scause, sepc, stval, stvec};
use crate::arch::riscv::kvm::context::{KvmRegs, VCpuContext};
use crate::arch::riscv::kvm::csr::hedeleg;
use crate::arch::riscv::task::traphandle;
use crate::kernel::mm::MemAccessType;
use crate::kvm::VCpuExitReason;

#[repr(u32)]
enum RiscVVCpuExitReason {
    SBICall = 16,
}

pub struct VCpu {
    context: VCpuContext,
    pc: usize,
}

impl VCpu {
    pub fn new() -> Self {
        Self {
            context: VCpuContext::new(),
            pc: 0,
        }
    }

    fn goto_guest(&mut self) {
        unsafe extern "C" {
            // In `clib/src/arch/riscv/kvm/guesttrap.S`.
            fn asm_kvm_guest_trap_entry();
            fn asm_kvm_guest_trap_return(context: *mut VCpuContext);
        }

        self.delegate_exceptions_to_vs();
        stvec::write(asm_kvm_guest_trap_entry as usize);
        sepc::write(self.pc);
        traphandle::restore_float_registers(&mut self.context.fpregs_mut());

        unsafe {
            asm_kvm_guest_trap_return(&mut self.context);
        };

        traphandle::set_stvec_to_kerneltrap_handler();
        traphandle::save_float_registers(&mut self.context.fpregs_mut());

        self.pc = sepc::read();
    }

    fn delegate_exceptions_to_vs(&self) {
        // Keep guest-page-fault exceptions in HS-mode so the host can lazily map guest memory.
        hedeleg::Hedeleg::clear()
            .delegate(Trap::InstAddrMisaligned)
            .delegate(Trap::InstAccessFault)
            .delegate(Trap::IllegalInst)
            .delegate(Trap::Breakpoint)
            .delegate(Trap::LoadAddrMisaligned)
            .delegate(Trap::LoadAccessFault)
            .delegate(Trap::StoreAddrMisaligned)
            .delegate(Trap::StoreAccessFault)
            .delegate(Trap::EcallU)
            .delegate(Trap::EcallS)
            .delegate(Trap::InstPageFault)
            .delegate(Trap::LoadPageFault)
            .delegate(Trap::StorePageFault)
            .delegate(Trap::DoubleTrap)
            .delegate(Trap::SoftwareCheck)
            .delegate(Trap::HardwareError)
            .write();
    }

    pub fn run(&mut self) -> VCpuExitReason {
        loop {
            self.goto_guest();

            match scause::cause() {
                Cause::Trap(trap) => match trap {
                    Trap::InstGuestPageFault => {
                        let addr = stval::read();
                        let addr = if addr == 0 { self.pc } else { addr };
                        return VCpuExitReason::MemoryFault(addr, MemAccessType::Execute);
                    }
                    Trap::LoadGuestPageFault => {
                        let addr = stval::read();
                        return VCpuExitReason::MemoryFault(addr, MemAccessType::Read);
                    }
                    Trap::StoreGuestPageFault => {
                        let addr = stval::read();
                        return VCpuExitReason::MemoryFault(addr, MemAccessType::Write);
                    }
                    Trap::EcallVS => return VCpuExitReason::ReturnToUser(RiscVVCpuExitReason::SBICall as usize),
                    _ => unreachable!("Unsupported trap cause: {:?}", trap),
                },

                Cause::Interrupt(interrupt) => {
                    traphandle::handle_interrupt(interrupt);
                }
            }
        }
    }

    pub fn regs(&self) -> KvmRegs {
        self.context.regs(self.pc)
    }

    pub fn set_regs(&mut self, regs: KvmRegs) {
        self.pc = regs.pc;
        self.context.set_regs(regs);
    }
}

unsafe impl Send for VCpu {}
