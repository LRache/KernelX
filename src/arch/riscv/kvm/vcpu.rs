use crate::arch::riscv::KvmPageTable;
use crate::arch::riscv::csr::scause::{Cause, Interrupt, Trap};
use crate::arch::riscv::csr::{Sstatus, SstatusSPP, scause, sepc, sscratch, stval, stvec};
use crate::arch::riscv::kvm::context::{KvmRegs, KvmSRegs, VCpuContext};
use crate::arch::riscv::kvm::csr::hcounteren::Hcounteren;
use crate::arch::riscv::kvm::csr::henvcfg::{Henvcfg, HenvcfgFlag};
use crate::arch::riscv::kvm::csr::hie::Hie;
use crate::arch::riscv::kvm::csr::hstatus::{Hstatus, HstatusSpv};
use crate::arch::riscv::kvm::csr::hvip::Hvip;
use crate::arch::riscv::kvm::csr::{VirtualInterrupt, hedeleg, hgatp, hideleg, htinst, htval, vsatp, vstimecmp};
use crate::arch::riscv::task::traphandle;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::MemAccessType;
use crate::kernel::scheduler::current;
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
    vsstatus: usize,
    vstvec: usize,
    vsscratch: usize,
    vsepc: usize,
    vscause: usize,
    vstval: usize,
    scounteren: usize,
    senvcfg: usize,
    pub(super) spp: SstatusSPP,
    // With hideleg configured below, hie also holds the vsie enable bits.
    pub(super) hie: Hie,
    // hvip.VSSIP also preserves the guest-writable vsip.SSIP bit. The
    // timer/external pending bits in vsip are derived, not separate state.
    pub(super) hvip: Hvip,
    pub(super) vstimecmp: usize,
    running: bool,
}

pub struct VCpu {
    pub(super) state: SleepLock<VCpuState>,
}

pub struct VCpuRunGuard<'a> {
    state: &'a SleepLock<VCpuState>,
}

impl Drop for VCpuRunGuard<'_> {
    fn drop(&mut self) {
        self.state.lock().running = false;
    }
}

impl VCpuState {
    fn new() -> Self {
        Self {
            context: VCpuContext::new(),
            pc: 0,
            vsatp: 0,
            vsstatus: 2usize << 32, // UXL=64; interrupts and extension state start disabled.
            vstvec: 0,
            vsscratch: 0,
            vsepc: 0,
            vscause: 0,
            vstval: 0,
            scounteren: 0,
            senvcfg: 0,
            spp: SstatusSPP::Supervisor,
            hie: Hie::clear(),
            hvip: Hvip::clear(),
            vstimecmp: usize::MAX,
            running: false,
        }
    }

    fn goto_guest(&mut self, hgatp: usize) {
        // This path must not schedule between loading and saving hart-local
        // guest state. RUN enters with host interrupts disabled, and the guest
        // trap returns with SIE cleared before any host interrupt is handled.
        debug_assert!(
            !Sstatus::read().sie(),
            "guest context switch requires host interrupts disabled"
        );
        Self::delegate_exceptions_to_vs();
        Self::delegate_interrupts_to_vs();
        Self::enable_sstc_timer();
        Sstatus::read().set_spie(false).set_spp(self.spp).write();
        Hstatus::read().set_spv(HstatusSpv::Virtual).write();

        stvec::write(asm_kvm_guest_trap_entry as *const () as usize);
        sepc::write(self.pc);
        sscratch::write(&raw mut self.context as usize);
        hgatp::write(hgatp);
        vsatp::write(self.vsatp);
        vstimecmp::write(self.vstimecmp);
        self.hie.write();
        self.hvip.write();

        let host_scounteren: usize;
        let host_senvcfg: usize;
        // SAFETY: RUN executes in HS-mode on an H-extension hart with host
        // interrupts disabled. These values belong to this exclusively running
        // vCPU. The shared supervisor CSRs are restored below before scheduling.
        unsafe {
            core::arch::asm!(
                "csrw vsstatus, {vsstatus}",
                "csrw vstvec, {vstvec}",
                "csrw vsscratch, {vsscratch}",
                "csrw vsepc, {vsepc}",
                "csrw vscause, {vscause}",
                "csrw vstval, {vstval}",
                "csrrw {host_scounteren}, scounteren, {scounteren}",
                "csrrw {host_senvcfg}, senvcfg, {senvcfg}",
                vsstatus = in(reg) self.vsstatus,
                vstvec = in(reg) self.vstvec,
                vsscratch = in(reg) self.vsscratch,
                vsepc = in(reg) self.vsepc,
                vscause = in(reg) self.vscause,
                vstval = in(reg) self.vstval,
                scounteren = in(reg) self.scounteren,
                senvcfg = in(reg) self.senvcfg,
                host_scounteren = out(reg) host_scounteren,
                host_senvcfg = out(reg) host_senvcfg,
                options(nostack),
            );
        }

        traphandle::restore_float_registers(&mut self.context.fpregs_mut());
        // SAFETY: context has the assembly-defined layout and stays live on
        // this kernel stack until the guest trap restores the host registers.
        // Host interrupts are disabled throughout the context switch.
        unsafe {
            asm_kvm_guest_trap_return(&mut self.context);
        };

        // SAFETY: The guest trap returned to HS-mode on the same hart with
        // interrupts disabled. Capture its CSRs and restore the shared host
        // values before any path can schedule or handle host interrupts.
        unsafe {
            core::arch::asm!(
                "csrr {vsstatus}, vsstatus",
                "csrr {vstvec}, vstvec",
                "csrr {vsscratch}, vsscratch",
                "csrr {vsepc}, vsepc",
                "csrr {vscause}, vscause",
                "csrr {vstval}, vstval",
                "csrrw {scounteren}, scounteren, {host_scounteren}",
                "csrrw {senvcfg}, senvcfg, {host_senvcfg}",
                vsstatus = out(reg) self.vsstatus,
                vstvec = out(reg) self.vstvec,
                vsscratch = out(reg) self.vsscratch,
                vsepc = out(reg) self.vsepc,
                vscause = out(reg) self.vscause,
                vstval = out(reg) self.vstval,
                scounteren = out(reg) self.scounteren,
                senvcfg = out(reg) self.senvcfg,
                host_scounteren = in(reg) host_scounteren,
                host_senvcfg = in(reg) host_senvcfg,
                options(nostack),
            );
        }

        traphandle::install_kerneltrap_handler();
        Hstatus::read().set_spv(HstatusSpv::Hypervisor).write();
        self.vsatp = vsatp::read();
        self.vstimecmp = vstimecmp::read();
        self.hie = Hie::read();
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

    fn enable_sstc_timer() {
        Hcounteren::read().set_tm(true).write();
        Henvcfg::read().set(HenvcfgFlag::STCE, true).write();
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

    pub fn enter_run(&self) -> SysResult<VCpuRunGuard<'_>> {
        let mut state = self.state.lock();
        if state.running {
            return Err(Errno::EBUSY);
        }
        state.running = true;
        Ok(VCpuRunGuard { state: &self.state })
    }

    // The guard keeps context setters from racing with an active KVM_RUN ioctl.
    pub fn run(
        &self,
        _run_guard: &VCpuRunGuard<'_>,
        pagetable: &SpinLock<KvmPageTable>,
        interrupt_state: KvmInterruptState,
    ) -> VCpuExitReason {
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
                        let val = htval::read();
                        let addr = val << 2 | stval::read() & 0x3;
                        let addr = if addr == 0 { state.pc } else { addr };
                        return VCpuExitReason::MemoryFault {
                            addr,
                            access_type: MemAccessType::Execute,
                            inst,
                            val,
                        };
                    }
                    Trap::LoadGuestPageFault => {
                        let inst = htinst::read();
                        let val = htval::read();
                        let addr = val << 2 | stval::read() & 0x3;
                        return VCpuExitReason::MemoryFault {
                            addr,
                            access_type: MemAccessType::Read,
                            inst,
                            val,
                        };
                    }
                    Trap::StoreGuestPageFault => {
                        let inst = htinst::read();
                        let val = htval::read();
                        let addr = val << 2 | stval::read() & 0x3;
                        return VCpuExitReason::MemoryFault {
                            addr,
                            access_type: MemAccessType::Write,
                            inst,
                            val,
                        };
                    }
                    Trap::EcallVS => {
                        if self.handle_sbi_call() {
                            continue;
                        }
                        return VCpuExitReason::ReturnToUser {
                            exit_code: RiscVVCpuExitReason::SBICall as usize,
                            inst: htinst::read(),
                            val: htval::read(),
                        };
                    }
                    _ => unreachable!("Unsupported trap cause: {:?}, stval={:#x}", trap, stval::read()),
                },

                Cause::Interrupt(Interrupt::Timer) => {
                    return VCpuExitReason::Timer;
                }
                Cause::Interrupt(interrupt) => {
                    traphandle::handle_interrupt(interrupt);
                    current::schedule();
                }
            }
        }
    }

    pub fn regs(&self) -> KvmRegs {
        self.state.lock().regs()
    }

    pub fn sregs(&self) -> KvmSRegs {
        self.state.lock().sregs()
    }

    pub fn set_regs(&self, regs: KvmRegs) -> SysResult<()> {
        let mut state = self.state.lock();
        if state.running {
            return Err(Errno::EBUSY);
        }
        state.set_regs(regs);
        Ok(())
    }

    pub fn gpr(&self, index: usize) -> Option<usize> {
        let state = self.state.lock();
        if index == 0 {
            Some(state.pc)
        } else {
            state.gpr().get(index).copied()
        }
    }

    pub fn set_gpr(&self, index: usize, value: usize) -> SysResult<()> {
        let mut state = self.state.lock();
        if state.running {
            return Err(Errno::EBUSY);
        }
        if index == 0 {
            state.pc = value;
            return Ok(());
        }
        *state.gpr_mut().get_mut(index).ok_or(Errno::EINVAL)? = value;
        Ok(())
    }

    pub fn set_interrupt_pending(&self, kind: KvmInterruptKind) {
        self.state.lock().set_interrupt_pending(kind);
    }

    pub fn clear_interrupt_pending(&self, kind: KvmInterruptKind) {
        self.state.lock().clear_interrupt_pending(kind);
    }
}

unsafe impl Send for VCpu {}
