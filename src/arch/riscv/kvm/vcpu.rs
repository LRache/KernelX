use crate::arch::riscv::KvmPageTable;
use crate::arch::riscv::csr::scause::{Cause, Interrupt, Trap};
use crate::arch::riscv::csr::{Sstatus, SstatusSPP, scause, sepc, sscratch, stval, stvec};
use crate::arch::riscv::kvm::context::{KvmRegs, KvmSRegs, VCpuContext};
use crate::arch::riscv::kvm::csr::hcounteren::Hcounteren;
use crate::arch::riscv::kvm::csr::hstatus::{Hstatus, HstatusSpv};
use crate::arch::riscv::kvm::csr::hvip::Hvip;
use crate::arch::riscv::kvm::csr::{
    VirtualInterrupt, hcounteren, hedeleg, hgatp, hideleg, htinst, htval, vsatp, vstimecmp,
};
use crate::arch::riscv::task::traphandle;
use crate::kernel::mm::MemAccessType;
use crate::klib::{SleepLock, SpinLock};
use crate::kvm::{KvmInterruptKind, KvmInterruptState, VCpuExitReason};

unsafe extern "C" {
    // In `clib/src/arch/riscv/kvm/guesttrap.S`.
    fn asm_kvm_guest_trap_entry();
    fn asm_kvm_guest_trap_return(context: *mut VCpuContext);
}

#[repr(u32)]
enum RiscVVCpuExitReason {
    SBICall = 16,
}

#[derive(Clone, Copy)]
pub struct VCpuState {
    pub(super) context: VCpuContext,
    pub(super) pc: usize,
    pub(super) vsatp: usize,
    pub(super) spp: SstatusSPP,
    pub(super) hvip: Hvip,
    pub(super) vstimecmp: usize,
}

pub struct VCpu {
    pub(super) state: SleepLock<VCpuState>,
}

impl VCpuState {
    fn new() -> Self {
        Self {
            context: VCpuContext::new(),
            pc: 0,
            vsatp: 0,
            spp: SstatusSPP::Supervisor,
            hvip: Hvip::clear(),
            vstimecmp: usize::MAX,
        }
    }

    fn goto_guest(&mut self, hgatp: usize) {
        Self::delegate_exceptions_to_vs();
        Self::delegate_interrupts_to_vs();
        self.enable_guest_counters();
        Hcounteren::read().set_tm(true).write();
        Sstatus::read().set_spie(false).set_spp(self.spp).write();
        Hstatus::read().set_spv(HstatusSpv::Virtual).write();

        stvec::write(asm_kvm_guest_trap_entry as usize);
        sepc::write(self.pc);
        sscratch::write(&raw mut self.context as usize);
        hgatp::write(hgatp);
        vsatp::write(self.vsatp);
        vstimecmp::write(self.vstimecmp);
        self.hvip.write();

        traphandle::restore_float_registers(&mut self.context.fpregs_mut());
        unsafe {
            asm_kvm_guest_trap_return(&mut self.context);
        };

        Hstatus::read().set_spv(HstatusSpv::Hypervisor).write();
        traphandle::set_stvec_to_kerneltrap_handler();
        self.vsatp = vsatp::read();
        self.hvip = Hvip::read();
        self.spp = Sstatus::read().spp();
        traphandle::save_float_registers(&mut self.context.fpregs_mut());

        self.pc = sepc::read();
    }

    fn delegate_exceptions_to_vs() {
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

    fn delegate_interrupts_to_vs() {
        hideleg::Hideleg::clear()
            .delegate(VirtualInterrupt::Software)
            .delegate(VirtualInterrupt::Timer)
            .delegate(VirtualInterrupt::External)
            .write();
    }

    fn enable_guest_counters(&self) {
        hcounteren::Hcounteren::read().set_tm(true).write();
    }

    pub(super) fn regs(&self) -> KvmRegs {
        self.context.regs(self.pc)
    }

    fn sregs(&self) -> KvmSRegs {
        KvmSRegs { satp: self.vsatp }
    }

    pub(super) fn set_regs(&mut self, regs: KvmRegs) {
        self.pc = regs.pc;
        self.context.set_regs(regs);
    }

    pub(super) fn set_vstimecmp(&mut self, time: usize) {
        self.vstimecmp = time;
    }

    pub(super) fn gpr(&self) -> &[usize; 32] {
        self.context.gpr()
    }

    pub(super) fn gpr_mut(&mut self) -> &mut [usize; 32] {
        self.context.gpr_mut()
    }

    fn virtual_interrupt(kind: KvmInterruptKind) -> VirtualInterrupt {
        match kind {
            KvmInterruptKind::Timer => VirtualInterrupt::Timer,
            KvmInterruptKind::Hardware => VirtualInterrupt::External,
        }
    }

    fn set_interrupt_pending(&mut self, kind: KvmInterruptKind) {
        self.hvip.set_pending(Self::virtual_interrupt(kind), true);
    }

    fn clear_interrupt_pending(&mut self, kind: KvmInterruptKind) {
        self.hvip.set_pending(Self::virtual_interrupt(kind), false);
    }

    fn set_interrupt_state(&mut self, interrupt_state: KvmInterruptState) {
        if interrupt_state.timer {
            self.set_interrupt_pending(KvmInterruptKind::Timer);
        } else {
            self.clear_interrupt_pending(KvmInterruptKind::Timer);
        }

        if interrupt_state.hardware {
            self.set_interrupt_pending(KvmInterruptKind::Hardware);
        } else {
            self.clear_interrupt_pending(KvmInterruptKind::Hardware);
        }
    }
}

impl VCpu {
    pub fn new() -> Self {
        Self {
            state: SleepLock::new(VCpuState::new(), "VCpu::state"),
        }
    }

    pub fn run(&self, pagetable: &SpinLock<KvmPageTable>, interrupt_state: KvmInterruptState) -> VCpuExitReason {
        loop {
            let mut state = *self.state.lock();
            state.set_interrupt_state(interrupt_state);
            let hgatp = pagetable.lock().get_hgatp();
            state.goto_guest(hgatp);
            *self.state.lock() = state;

            match scause::cause() {
                Cause::Trap(trap) => match trap {
                    Trap::InstGuestPageFault => {
                        let inst = htinst::read();
                        let addr = htval::read() << 2 | stval::read() & 0x3;
                        let addr = if addr == 0 { state.pc } else { addr };
                        return VCpuExitReason::MemoryFault(addr, MemAccessType::Execute, inst);
                    }
                    Trap::LoadGuestPageFault => {
                        let inst = htinst::read();
                        let addr = htval::read() << 2 | stval::read() & 0x3;
                        return VCpuExitReason::MemoryFault(addr, MemAccessType::Read, inst);
                    }
                    Trap::StoreGuestPageFault => {
                        let inst = htinst::read();
                        let addr = htval::read() << 2 | stval::read() & 0x3;
                        return VCpuExitReason::MemoryFault(addr, MemAccessType::Write, inst);
                    }
                    Trap::EcallVS => {
                        if self.handle_sbi_call() {
                            continue;
                        }
                        return VCpuExitReason::ReturnToUser(RiscVVCpuExitReason::SBICall as usize);
                    }
                    _ => unreachable!("Unsupported trap cause: {:?}, stval={:#x}", trap, stval::read()),
                },

                Cause::Interrupt(Interrupt::Timer) => return VCpuExitReason::Timer,
                Cause::Interrupt(interrupt) => traphandle::handle_interrupt(interrupt),
            }
        }
    }

    pub fn regs(&self) -> KvmRegs {
        self.state.lock().regs()
    }

    pub fn sregs(&self) -> KvmSRegs {
        self.state.lock().sregs()
    }

    pub fn set_regs(&self, regs: KvmRegs) {
        self.state.lock().set_regs(regs);
    }

    pub fn set_interrupt_pending(&self, kind: KvmInterruptKind) {
        self.state.lock().set_interrupt_pending(kind);
    }

    pub fn clear_interrupt_pending(&self, kind: KvmInterruptKind) {
        self.state.lock().clear_interrupt_pending(kind);
    }
}

unsafe impl Send for VCpu {}
