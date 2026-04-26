use crate::arch::riscv::KvmPageTable;
use crate::arch::riscv::csr::scause::{Cause, Trap};
use crate::arch::riscv::csr::{Sstatus, SstatusSPP, scause, sepc, sscratch, stval, stvec};
use crate::arch::riscv::kvm::context::{KvmRegs, KvmSRegs, VCpuContext};
use crate::arch::riscv::kvm::csr::hstatus::{Hstatus, HstatusSpv};
use crate::arch::riscv::kvm::csr::{hedeleg, hgatp, htinst, vsatp};
use crate::arch::riscv::task::traphandle;
use crate::kernel::mm::MemAccessType;
use crate::klib::SpinLock;
use crate::kvm::VCpuExitReason;

unsafe extern "C" {
    // In `clib/src/arch/riscv/kvm/guesttrap.S`.
    fn asm_kvm_guest_trap_entry();
    fn asm_kvm_guest_trap_return(context: *mut VCpuContext);
}

#[repr(u32)]
enum RiscVVCpuExitReason {
    SBICall = 16,
}

pub struct VCpu {
    context: VCpuContext,
    pc: usize,
    vsatp: usize,
    spp: SstatusSPP,
}

impl VCpu {
    pub fn new() -> Self {
        Self {
            context: VCpuContext::new(),
            pc: 0,
            vsatp: 0,
            spp: SstatusSPP::Supervisor,
        }
    }

    fn goto_guest(&mut self, hgatp: usize) {
        self.delegate_exceptions_to_vs();
        stvec::write(asm_kvm_guest_trap_entry as usize);
        sepc::write(self.pc);
        sscratch::write(&raw mut self.context as usize);
        hgatp::write(hgatp);
        Sstatus::read().set_spie(false).set_spp(self.spp).write();
        Hstatus::read().set_spv(HstatusSpv::Virtual).write();
        vsatp::write(self.vsatp);
        traphandle::restore_float_registers(&mut self.context.fpregs_mut());

        unsafe {
            asm_kvm_guest_trap_return(&mut self.context);
        };

        Hstatus::read().set_spv(HstatusSpv::Hypervisor).write();
        traphandle::set_stvec_to_kerneltrap_handler();
        self.vsatp = vsatp::read();
        self.spp = Sstatus::read().spp();
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

    pub fn run(&mut self, pagetable: &SpinLock<KvmPageTable>) -> VCpuExitReason {
        loop {
            let hgatp = pagetable.lock().get_hgatp();
            self.goto_guest(hgatp);

            // crate::kdebug!("Guest trap: {:?}", scause::cause());
            match scause::cause() {
                Cause::Trap(trap) => match trap {
                    Trap::InstGuestPageFault => {
                        let inst = htinst::read();
                        let addr = stval::read();
                        let addr = if addr == 0 { self.pc } else { addr };
                        return VCpuExitReason::MemoryFault(addr, MemAccessType::Execute, inst);
                    }
                    Trap::LoadGuestPageFault => {
                        let inst = htinst::read();
                        let addr = stval::read();
                        return VCpuExitReason::MemoryFault(addr, MemAccessType::Read, inst);
                    }
                    Trap::StoreGuestPageFault => {
                        let inst = htinst::read();
                        let addr = stval::read();
                        return VCpuExitReason::MemoryFault(addr, MemAccessType::Write, inst);
                    }
                    Trap::EcallVS => {
                        return VCpuExitReason::ReturnToUser(RiscVVCpuExitReason::SBICall as usize);
                    }
                    _ => unreachable!("Unsupported trap cause: {:?}, stval={:#x}", trap, stval::read()),
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

    pub fn sregs(&self) -> KvmSRegs {
        KvmSRegs { satp: self.vsatp }
    }

    pub fn set_regs(&mut self, regs: KvmRegs) {
        self.pc = regs.pc;
        self.context.set_regs(regs);
    }
}

unsafe impl Send for VCpu {}
