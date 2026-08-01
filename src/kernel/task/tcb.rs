use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::time::Duration;

use crate::arch::{KernelContext, UserContext, UserContextTrait};
use crate::driver::chosen::kclock;
use crate::fs::file::{FileFlags, FileOps, RandomAccessFile};
use crate::fs::{Inode, Perm, PermFlags, vfs};
use crate::kernel::config::UTASK_KSTACK_PAGE_COUNT;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, FanotifyEventMask, notify_fanotify, timer, wait_fanotify_open_exec_permission};
use crate::kernel::ipc::{PendingSignal, SignalNum, SignalSet, SignalStackState};
use crate::kernel::mm::maparea::{AuxKey, Auxv};
use crate::kernel::mm::{AddrSpace, elf};
#[cfg(feature = "watchdog")]
use crate::kernel::scheduler::TaskRunState;
// PERF_DEBUG(scheduler-time): Temporary blocked-time stamps.
#[cfg(feature = "scheduler-block-reason-debug")]
use crate::kernel::scheduler::time_debug::{self, DebugBlockStamp};
use crate::kernel::scheduler::{KernelStack, Task, Tid, WakeupAction, WakeupFailure, current};
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::fdtable::{FDFlags, FDTable};
use crate::kernel::task::{PCB, WaitStatus, manager};
use crate::kernel::uapi::Uid;
use crate::kernel::usync::futex;
use crate::kernel::{config, scheduler};
use crate::klib::ksync::TaskLocal;
use crate::klib::{RWLock, SleepLock, SpinLock};
use crate::{arch, ktrace};

#[cfg(target_arch = "loongarch64")]
const HWCAP_LOONGARCH_UAL: usize = 1 << 2;

#[derive(Debug, Clone, Copy)]
pub struct PtraceStop {
    /// This signal will be reported to the tracer when waits.
    pub signum: SignalNum,

    /// The signal which caused the ptrace stop,
    /// this is used for ptrace signal injection.
    pub original_signal: Option<PendingSignal>,

    /// Whether this ptrace stop has been reported to the tracer.
    /// This is used to ensure that each ptrace stop is only reported once.
    reported: bool,
}

#[derive(Debug, Clone, Copy)]
enum PendingTCBStateChange {
    None,
    Stop(SignalNum),
    PtraceStop(PtraceStop),
    Exit,
}

#[derive(Debug, Clone, Copy)]
pub enum TCBState {
    /// The task is currently running on a CPU.
    Running,

    /// The task is ready to run and can be scheduled on a CPU.
    Ready,

    /// The task is blocked, waiting for an event.
    Blocked,

    /// The task is blocked and cannot be interrupted until the event it is waiting for occurs.
    BlockedUninterruptible,

    /// The task has been stopped by a job-control signal and waits for SIGCONT.
    Stopped,

    /// The task has been stopped by ptrace and waits for tracer resume.
    PtraceStop(PtraceStop),

    /// The task has exited. This state MUST BE set by the task itself.
    Exited,
}

impl PartialEq for TCBState {
    fn eq(&self, other: &Self) -> bool {
        core::mem::discriminant(self) == core::mem::discriminant(other)
    }
}

impl Eq for TCBState {}

#[cfg(feature = "watchdog")]
fn task_run_state(state: TCBState) -> TaskRunState {
    match state {
        TCBState::Ready => TaskRunState::Ready,
        TCBState::Running => TaskRunState::Running,
        TCBState::Blocked | TCBState::BlockedUninterruptible => TaskRunState::Blocked,
        TCBState::Stopped | TCBState::PtraceStop(_) | TCBState::Exited => TaskRunState::Other,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TCBStateSet {
    state: TCBState,
    pending_state_change: PendingTCBStateChange,
    on_cpu: bool,
    wake_pending: bool,
    // PERF_DEBUG(scheduler-time): Temporary pending block name and switch-out stamp.
    #[cfg(feature = "scheduler-block-reason-debug")]
    debug_block_name: Option<&'static str>,
    #[cfg(feature = "scheduler-block-reason-debug")]
    debug_block_stamp: Option<DebugBlockStamp>,
    #[cfg(feature = "watchdog")]
    watchdog_counted: bool,

    pub pending_signal: Option<PendingSignal>,
    pub signal_to_wait: SignalSet,
}

impl TCBStateSet {
    pub fn state(&self) -> TCBState {
        self.state
    }

    pub fn is_dead(&self) -> bool {
        matches!(self.pending_state_change, PendingTCBStateChange::Exit)
    }

    pub fn stop_pending(&self) -> bool {
        matches!(
            self.pending_state_change,
            PendingTCBStateChange::Stop(_) | PendingTCBStateChange::PtraceStop(_)
        )
    }

    fn set_state(&mut self, state: TCBState) {
        #[cfg(feature = "watchdog")]
        if self.watchdog_counted {
            scheduler::watchdog::transition_task(task_run_state(self.state), task_run_state(state));
        }
        self.state = state;
    }

    fn make_ready(&mut self) -> WakeupAction {
        // PERF_DEBUG(scheduler-time): Close the blocked interval before wakeup.
        #[cfg(feature = "scheduler-block-reason-debug")]
        {
            time_debug::finish_block(self.debug_block_stamp.take());
        }
        #[cfg(feature = "scheduler-block-reason-debug")]
        {
            self.debug_block_name = None;
        }
        if self.on_cpu {
            self.wake_pending = true;
            WakeupAction::Deferred
        } else {
            self.set_state(TCBState::Ready);
            WakeupAction::Enqueue
        }
    }
}

impl Default for TCBStateSet {
    fn default() -> Self {
        Self {
            state: TCBState::Ready,
            pending_state_change: PendingTCBStateChange::None,
            on_cpu: false,
            wake_pending: false,
            // PERF_DEBUG(scheduler-time): Temporary blocked-time state.
            #[cfg(feature = "scheduler-block-reason-debug")]
            debug_block_name: None,
            #[cfg(feature = "scheduler-block-reason-debug")]
            debug_block_stamp: None,
            #[cfg(feature = "watchdog")]
            watchdog_counted: false,
            pending_signal: None,
            signal_to_wait: SignalSet::empty(),
        }
    }
}

pub struct TimeCounter {
    pub user_time: Duration,
    pub system_time: Duration,
    pub user_start: Option<Duration>,
    pub system_start: Option<Duration>,
}

impl TimeCounter {
    pub fn new() -> Self {
        Self {
            user_time: Duration::ZERO,
            system_time: Duration::ZERO,
            user_start: None,
            system_start: Some(timer::now()),
        }
    }

    fn pause_system_time(&mut self, now: Duration) -> Duration {
        if let Some(system_start) = self.system_start.take() {
            let delta = now.checked_sub(system_start).unwrap_or(Duration::ZERO);
            self.system_time += delta;
            delta
        } else {
            Duration::ZERO
        }
    }

    fn resume_system_time(&mut self, now: Duration) {
        self.system_start = Some(now);
    }
}

pub struct Tracer {
    pub pcb: Arc<PCB>,
    pub tid: Tid,
}

impl Clone for Tracer {
    fn clone(&self) -> Self {
        Self {
            pcb: self.pcb.clone(),
            tid: self.tid,
        }
    }
}

pub struct TCB {
    tid: Tid,
    create_time: Duration,
    parent: Arc<PCB>,
    pub tracer: SpinLock<Option<Tracer>>,
    ptrace_signal_delivery: SpinLock<Option<SignalNum>>,
    tid_address: SpinLock<Option<usize>>,
    pub robust_list: SpinLock<Option<usize>>,

    user_context: TaskLocal<Box<UserContext>>,
    kernel_context: UnsafeCell<KernelContext>,
    pub kernel_stack: KernelStack<{ UTASK_KSTACK_PAGE_COUNT - 1 }>,

    addrspace: Arc<AddrSpace>,
    fdtable: RWLock<Arc<SleepLock<FDTable>>>,

    pub signal_mask: SpinLock<SignalSet>,
    signal_stack: SpinLock<SignalStackState>,
    ucontext_syscall_retreg_backup: TaskLocal<Option<usize>>,

    state: SpinLock<TCBStateSet>,
    pub wakeup_event: SpinLock<Option<Event>>,
    parent_waiting_vfork: SpinLock<Option<Arc<dyn Task>>>,
    pub time_counter: SpinLock<TimeCounter>,

    #[cfg(feature = "lockdep")]
    lockstate: crate::klib::ksync::LockState,
}

impl TCB {
    pub fn new(
        tid: i32,
        parent: &Arc<PCB>,

        mut user_context: UserContext,

        addrspace: Arc<AddrSpace>,
        fdtable: Arc<SleepLock<FDTable>>,
    ) -> Arc<Self> {
        let kernel_stack = KernelStack::new();
        user_context.set_kernel_stack_top(kernel_stack.get_top());

        user_context.set_addrspace(&addrspace);
        addrspace.acquire_task_user();

        let tcb = Arc::new(Self {
            tid,
            create_time: kclock::now().unwrap_or(Duration::ZERO),
            parent: parent.clone(),
            tracer: SpinLock::new(None, "TCB::tracer"),
            ptrace_signal_delivery: SpinLock::new(None, "TCB::ptrace_signal_delivery"),
            tid_address: SpinLock::new(None, "TCB::tid_address"),
            robust_list: SpinLock::new(None, "TCB::robust_list"),

            user_context: TaskLocal::new(tid, Box::new(user_context)),
            kernel_context: UnsafeCell::new(KernelContext::new_user(&kernel_stack, &addrspace)),
            kernel_stack,

            addrspace,
            fdtable: RWLock::new(fdtable, "TCB::fdtable"),

            signal_mask: SpinLock::new(SignalSet::empty(), "TCB::signal_mask"),
            signal_stack: SpinLock::new(SignalStackState::default(), "TCB::signal_stack"),
            ucontext_syscall_retreg_backup: TaskLocal::new(tid, None),

            state: SpinLock::new(TCBStateSet::default(), "TCB::state"),
            wakeup_event: SpinLock::new(None, "TCB::wakeup_event"),
            parent_waiting_vfork: SpinLock::new(None, "TCB::parent_waiting_vfork"),
            time_counter: SpinLock::new(TimeCounter::new(), "TCB::time_counter"),
            #[cfg(feature = "lockdep")]
            lockstate: crate::klib::ksync::LockState::new(),
        });

        tcb
    }

    pub fn new_inittask(
        tid: i32,
        parent: &Arc<PCB>,
        initpath: &str,
        argv: &[&str],
        envp: &[&str],
        tty: &str,
    ) -> (Arc<Self>, String, Arc<Inode>) {
        let exec_perm = Perm::new(0, 0, PermFlags::R | PermFlags::X);
        let file = vfs::open_file(initpath, FileFlags::readonly(), &exec_perm)
            .expect("Failed to open init file")
            .downcast_arc::<RandomAccessFile>()
            .map_err(|_| Errno::ENOEXEC)
            .expect("Failed to open init file as RandomAccessFile");

        // Read the shebang
        let mut first_line = [0u8; 128];
        let n = file
            .pread(&mut first_line, 0)
            .expect("Failed to read first line of init file");
        let first_line = core::str::from_utf8(&first_line[..n]).unwrap_or("");
        let first_line = first_line.lines().next().unwrap_or("");
        let first_line = first_line.trim_end_matches('\n');
        if first_line.starts_with("#!") {
            let shebang = first_line.trim_start_matches("#!").trim();
            let mut parts = shebang.split_whitespace();
            if let Some(interpreter) = parts.next() {
                let mut new_argv = vec![interpreter];
                for part in parts {
                    new_argv.push(part);
                }
                new_argv.push(initpath);
                for arg in argv.iter().skip(1) {
                    new_argv.push(arg);
                }

                return Self::new_inittask(tid, parent, interpreter, &new_argv, envp, tty);
            }
        }

        let exec_path = file.get_dentry().unwrap().get_path();
        let exec_inode = file.get_inode().cloned().expect("Init executable missing inode");
        exec_inode.begin_exec().expect("Failed to acquire init executable");

        let mut addrspace = AddrSpace::new();
        let (user_entry, dyn_info) = elf::loader::load_elf(vfs::get_root_dentry(), &file, &mut addrspace, &exec_perm)
            .expect("Failed to load ELF for init task");

        let mut auxv = Auxv::new();
        if let Some(dyn_info) = dyn_info {
            auxv.push(AuxKey::BASE, dyn_info.interpreter_base);
            auxv.push(AuxKey::PHDR, dyn_info.phdr_addr);
            auxv.push(AuxKey::PHENT, dyn_info.phent as usize);
            auxv.push(AuxKey::PHNUM, dyn_info.phnum as usize);
            auxv.push(AuxKey::FLAGS, 0);
            auxv.push(AuxKey::ENTRY, dyn_info.user_entry);
            ktrace!("Dynamic linker info: {:?}", dyn_info);
        }

        auxv.push(AuxKey::RANDOM, config::USER_RANDOM_ADDR_BASE);
        auxv.push(AuxKey::PAGESZ, arch::PGSIZE);
        #[cfg(target_arch = "loongarch64")]
        auxv.push(AuxKey::HWCAP, HWCAP_LOONGARCH_UAL);

        let userstack_top = addrspace
            .create_user_stack(argv, envp, &auxv)
            .expect("Failed to push args and envp to userstack");

        let tty = vfs::open_file(
            tty,
            FileFlags {
                readable: true,
                writable: true,
                blocked: true,
                append: false,
                direct: false,
            },
            &Perm::new(0, 0, PermFlags::R | PermFlags::W),
        )
        .expect("Failed to open tty for init task");

        let mut fdtable = FDTable::new(parent.pid());
        for _ in 0..3 {
            fdtable.push(tty.clone(), FDFlags::empty()).unwrap();
        }
        fdtable.push(file.clone(), FDFlags::empty()).unwrap();

        let mut user_context = UserContext::new();
        user_context.set_user_stack_top(userstack_top);
        user_context.set_user_entry(user_entry);
        // user_context.set_tls(config::USER_RANDOM_ADDR_BASE + arch::PGSIZE / 2);

        let tcb = Self::new(
            tid,
            parent,
            user_context,
            addrspace,
            Arc::new(SleepLock::new(fdtable, "TCB::fdtable")),
        );

        (tcb, exec_path, exec_inode)
    }

    pub fn new_clone(
        &self,
        tid: Tid,
        parent: &Arc<PCB>,
        userstack: usize,
        flags: &TaskCloneFlags,
        tls: Option<usize>,
    ) -> SysResult<Arc<Self>> {
        let mut new_user_context = self.user_context().new_clone();

        if userstack != 0 {
            new_user_context.set_user_stack_top(userstack);
        }

        let new_addrspace = if flags.vm {
            self.addrspace.clone()
        } else {
            let addrspace = self.addrspace.fork();
            new_user_context.set_addrspace(&addrspace);
            addrspace
        };

        new_user_context.skip_syscall_instruction();

        if let Some(tls) = tls {
            new_user_context.set_tls(tls);
        }

        let new_fdtable = if flags.files {
            self.fdtable()
        } else {
            let fdtable = self.fdtable();
            Arc::new(SleepLock::new(fdtable.lock().fork(parent.pid())?, "TCB::fdtable"))
        };

        let new_tcb = Self::new(tid, parent, new_user_context, new_addrspace, new_fdtable);
        new_tcb.set_signal_mask(self.get_signal_mask());
        if flags.vm && !flags.vfork {
            new_tcb.set_signal_stack_state(SignalStackState::default());
        } else {
            new_tcb.set_signal_stack_state(self.get_signal_stack_state());
        }

        Ok(new_tcb)
    }

    pub fn new_exec(
        &self,
        file: Arc<RandomAccessFile>,
        invoked_path: &str,
        argv: &[&str],
        envp: &[&str],
    ) -> SysResult<(Arc<Self>, String, Arc<Inode>)> {
        let fanotify_file: Arc<dyn FileOps> = file.clone();
        wait_fanotify_open_exec_permission(&fanotify_file)?;
        notify_fanotify(&fanotify_file, FanotifyEventMask::FAN_OPEN);
        notify_fanotify(&fanotify_file, FanotifyEventMask::FAN_OPEN_EXEC);

        // Read the shebang
        let mut first_line = [0u8; 128];
        let n = file.pread(&mut first_line, 0)?;
        let first_line = core::str::from_utf8(&first_line[..n]).unwrap_or("");
        let first_line = first_line.lines().next().unwrap_or("");
        let first_line = first_line.trim_end_matches('\n');
        if first_line.starts_with("#!") {
            let shebang = first_line.trim_start_matches("#!").trim();
            let mut parts = shebang.split_whitespace();
            if let Some(interpreter) = parts.next() {
                let mut new_argv = vec![interpreter];
                for part in parts {
                    new_argv.push(part);
                }
                new_argv.push(invoked_path);
                for arg in argv.iter().skip(1) {
                    new_argv.push(arg);
                }

                let root = self.parent.root();
                let interpreter_file = vfs::openat_file(
                    &root,
                    &root,
                    interpreter,
                    FileFlags::readonly(),
                    &Perm::current(PermFlags::X),
                )?
                .downcast_arc::<RandomAccessFile>()
                .map_err(|_| Errno::ENOEXEC)?;
                return self.new_exec(interpreter_file, interpreter, &new_argv, envp);
            }
        }

        // SAFETY: RandomAccessFile must have dentry and path.
        let exec_path = file.get_dentry().unwrap().get_path();
        let exec_inode = file.get_inode().ok_or(Errno::ENOEXEC)?;
        exec_inode.begin_exec()?;

        let result = (|| {
            let mut addrspace = AddrSpace::new();
            let exec_perm = Perm::current(PermFlags::X);
            let root = self.parent.root();
            let (user_entry, dyn_info) = elf::loader::load_elf(&root, &file, &mut addrspace, &exec_perm)?;

            let mut auxv = Auxv::new();
            if let Some(dyn_info) = dyn_info {
                auxv.push(AuxKey::BASE, dyn_info.interpreter_base);
                auxv.push(AuxKey::PHDR, dyn_info.phdr_addr);
                auxv.push(AuxKey::PHENT, dyn_info.phent as usize);
                auxv.push(AuxKey::PHNUM, dyn_info.phnum as usize);
                auxv.push(AuxKey::ENTRY, dyn_info.user_entry);
            }

            auxv.push(AuxKey::PAGESZ, arch::PGSIZE);
            auxv.push(AuxKey::RANDOM, config::USER_RANDOM_ADDR_BASE);
            #[cfg(target_arch = "loongarch64")]
            auxv.push(AuxKey::HWCAP, HWCAP_LOONGARCH_UAL);

            let usetstack_top = addrspace.create_user_stack(argv, envp, &auxv)?;

            let mut new_user_context = UserContext::new();
            new_user_context.set_user_stack_top(usetstack_top);
            new_user_context.set_user_entry(user_entry);
            // Provide a non-zero initial tp so early TLS accesses in the
            // dynamic linker (tp + negative offset) land in the read-only
            // random page instead of faulting at kernel-range addresses.
            // The real tp is set later via set_tid_address / set_thread_area.
            // new_user_context.set_tls(config::USER_RANDOM_ADDR_BASE + arch::PGSIZE / 2);

            let fdtable = self.fdtable();
            fdtable.lock().cloexec();

            let new_tcb = TCB::new(self.tid, &self.parent, new_user_context, addrspace, fdtable);
            new_tcb.set_signal_mask(self.get_signal_mask());
            let tracer = self.tracer.lock().clone();
            *new_tcb.tracer.lock() = tracer;

            Ok((new_tcb, exec_path, exec_inode.clone()))
        })();

        if result.is_err() {
            exec_inode.end_exec();
        }

        result
    }

    pub fn tid(&self) -> i32 {
        self.tid
    }

    pub fn get_user_context_ptr(&self) -> usize {
        self.user_context.get_mut().as_mut() as *mut UserContext as usize
    }

    pub fn parent(&self) -> &Arc<PCB> {
        &self.parent
    }

    pub fn user_context(&self) -> &mut UserContext {
        self.user_context.get_mut().as_mut()
    }

    pub fn get_addrspace(&self) -> &Arc<AddrSpace> {
        &self.addrspace
    }

    pub fn fdtable(&self) -> Arc<SleepLock<FDTable>> {
        self.fdtable.read().clone()
    }

    pub fn unshare_fdtable(&self) -> SysResult<()> {
        let fdtable = self.fdtable();
        let new_fdtable = Arc::new(SleepLock::new(fdtable.lock().fork(self.parent.pid())?, "TCB::fdtable"));
        *self.fdtable.write() = new_fdtable;
        Ok(())
    }

    pub fn get_signal_mask(&self) -> SignalSet {
        *self.signal_mask.lock()
    }

    pub fn set_signal_mask(&self, mask: SignalSet) {
        *self.signal_mask.lock() = mask.without_unblockable();
    }

    pub fn swap_signal_mask(&self, mask: SignalSet) -> SignalSet {
        let mut signal_mask = self.signal_mask.lock();
        let old_mask = *signal_mask;
        *signal_mask = mask.without_unblockable();
        old_mask
    }

    pub fn get_signal_stack_state(&self) -> SignalStackState {
        *self.signal_stack.lock()
    }

    pub fn set_signal_stack(&self, stack: Option<(usize, usize)>) {
        let mut signal_stack = self.signal_stack.lock();
        signal_stack.stack = stack;
        if stack.is_none() {
            signal_stack.on_stack = false;
        }
    }

    pub fn set_signal_stack_state(&self, state: SignalStackState) {
        *self.signal_stack.lock() = state;
    }

    pub fn set_tid_address(&self, addr: usize) {
        *self.tid_address.lock() = Some(addr);
    }

    pub fn is_traced(&self) -> bool {
        self.tracer.lock().is_some()
    }

    pub fn is_traced_by(&self, tracer: &Arc<PCB>) -> bool {
        self.tracer
            .lock()
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(&current.pcb, tracer))
    }

    pub fn is_traced_by_waiter(&self, tracer: &Arc<PCB>, wait_parent_tid: Option<Tid>) -> bool {
        self.tracer.lock().as_ref().is_some_and(|current| {
            Arc::ptr_eq(&current.pcb, tracer) && wait_parent_tid.map_or(true, |tid| current.tid == tid)
        })
    }

    pub fn ptrace_original_signal(&self) -> Option<PendingSignal> {
        match self.state.lock().state {
            TCBState::PtraceStop(stop) => stop.original_signal,
            _ => None,
        }
    }

    pub fn is_ptrace_stopped(&self) -> bool {
        matches!(self.state.lock().state, TCBState::PtraceStop(_))
    }

    pub fn set_ptrace_tracer(&self, new_tracer: Tracer) -> bool {
        let mut tracer_lock = self.tracer.lock();
        if tracer_lock.is_some() {
            false
        } else {
            *tracer_lock = Some(new_tracer);
            true
        }
    }

    pub fn ptrace_wait_status(&self, tracer: &Arc<PCB>, consume: bool) -> Option<super::pcb::WaitStatus> {
        if !self.is_traced_by(tracer) {
            return None;
        }

        let mut state = self.state.lock();
        let TCBState::PtraceStop(stop) = &mut state.state else {
            return None;
        };
        if stop.reported {
            return None;
        }
        if consume {
            stop.reported = true;
        }
        Some(WaitStatus::PtraceStopped(stop.signum))
    }

    pub fn request_ptrace_stop(&self, signum: SignalNum, original_signal: Option<PendingSignal>) -> bool {
        if !self.is_traced() {
            return false;
        }

        let mut state = self.state.lock();
        if matches!(state.pending_state_change, PendingTCBStateChange::Exit) || state.state == TCBState::Exited {
            return false;
        }
        if matches!(state.pending_state_change, PendingTCBStateChange::PtraceStop(_))
            || matches!(state.state, TCBState::PtraceStop(_))
        {
            return false;
        }

        let stop = PtraceStop {
            signum,
            original_signal,
            reported: false,
        };
        let already_stopped = state.state == TCBState::Stopped;
        state.pending_state_change = PendingTCBStateChange::PtraceStop(stop);
        if already_stopped {
            state.set_state(TCBState::PtraceStop(stop));
        }
        let should_wake = state.state == TCBState::Blocked;
        drop(state);

        if already_stopped {
            self.notify_ptrace_stopped(signum);
        }

        should_wake
    }

    pub fn resume_from_ptrace_stop(&self, signal: Option<PendingSignal>) -> SysResult<WakeupAction> {
        let mut state = self.state.lock();
        if !matches!(state.state, TCBState::PtraceStop(_)) {
            return Err(Errno::ESRCH);
        }

        if !matches!(state.pending_state_change, PendingTCBStateChange::Exit) {
            state.pending_state_change = PendingTCBStateChange::None;
        }

        if let Some(signal) = signal {
            *self.ptrace_signal_delivery.lock() = Some(signal.signum);
            if state.pending_signal.is_none() {
                state.pending_signal = Some(signal);
            } else {
                self.parent.pending_signals().lock().add_pending(signal)?;
            }
        }

        Ok(state.make_ready())
    }

    pub fn clear_ptrace_tracer(&self) {
        *self.tracer.lock() = None;
        let mut state = self.state.lock();
        if matches!(state.pending_state_change, PendingTCBStateChange::PtraceStop(_)) {
            state.pending_state_change = PendingTCBStateChange::None;
        }
    }

    pub fn take_ptrace_signal_delivery(&self, signum: SignalNum) -> bool {
        let mut delivery = self.ptrace_signal_delivery.lock();
        if *delivery == Some(signum) {
            *delivery = None;
            true
        } else {
            false
        }
    }

    fn notify_ptrace_stopped(&self, signum: SignalNum) {
        if let Some(tracer) = self.tracer.lock().clone() {
            tracer
                .pcb
                .notify_ptrace_stopped(self.tid, self.parent.uid(), signum, tracer.tid);
        }
    }

    pub fn state(&self) -> &SpinLock<TCBStateSet> {
        &self.state
    }

    #[cfg(feature = "watchdog")]
    pub(super) fn register_watchdog(&self) {
        let mut state = self.state.lock();
        if state.watchdog_counted {
            return;
        }
        scheduler::watchdog::register_task(task_run_state(state.state));
        state.watchdog_counted = true;
    }

    #[cfg(feature = "watchdog")]
    pub(super) fn unregister_watchdog(&self) {
        let mut state = self.state.lock();
        if !state.watchdog_counted {
            return;
        }
        scheduler::watchdog::unregister_task(task_run_state(state.state));
        state.watchdog_counted = false;
    }

    pub fn block_if_no_pending_signal(&self, _reason: &str) -> bool {
        debug_assert!(current::tid() == self.tid);

        let mut state = self.state.lock();
        if state.pending_signal.is_some() {
            return false;
        }
        match state.state {
            TCBState::Ready | TCBState::Running => {}
            _ => return false,
        }
        debug_assert!(state.on_cpu);
        debug_assert!(!state.wake_pending);
        state.set_state(TCBState::Blocked);
        true
    }

    pub fn prepare_signal_wait(&self, signal_set: SignalSet, _reason: &str) -> SysResult<Option<PendingSignal>> {
        debug_assert!(current::tid() == self.tid);

        let mut state = self.state.lock();
        if let Some(pending) = state.pending_signal {
            if signal_set.contains(pending.signum) {
                return Ok(state.pending_signal.take());
            }
            return Err(Errno::EINTR);
        }
        if let Some(pending) = self.parent.pending_signals().lock().pop_waiting(signal_set, self.tid) {
            return Ok(Some(pending));
        }
        match state.state {
            TCBState::Ready | TCBState::Running => {}
            _ => return Err(Errno::EINTR),
        }
        debug_assert!(state.on_cpu);
        debug_assert!(!state.wake_pending);
        state.signal_to_wait = signal_set;
        state.set_state(TCBState::Blocked);
        Ok(None)
    }

    pub fn create_time(&self) -> Duration {
        self.create_time
    }

    pub fn thread_cpu_time(&self) -> Duration {
        let now = timer::now();
        let counter = self.time_counter.lock();
        let mut cpu_time = counter.user_time + counter.system_time;
        if current::has_task() && current::tid() == self.tid {
            if let Some(user_start) = counter.user_start {
                cpu_time += now.checked_sub(user_start).unwrap_or(Duration::ZERO);
            }
            if let Some(system_start) = counter.system_start {
                cpu_time += now.checked_sub(system_start).unwrap_or(Duration::ZERO);
            }
        }
        cpu_time
    }

    pub fn check_cpu_timers(&self) {
        self.parent.timers.check_process_cpu();
        self.parent.timers.check_thread_cpu(self.tid);
    }

    fn pause_user_task_system_time(&self) {
        let delta = self.time_counter.lock().pause_system_time(timer::now());
        self.parent.add_task_time(Duration::ZERO, delta);
        self.check_cpu_timers();
    }

    fn resume_user_task_system_time(&self) {
        self.time_counter.lock().resume_system_time(timer::now());
    }

    pub fn set_dead(&self) {
        let mut state = self.state.lock();
        state.pending_state_change = PendingTCBStateChange::Exit;
    }

    pub fn request_stop_from_signal(&self, signum: SignalNum) -> bool {
        let mut state = self.state.lock();
        if matches!(state.pending_state_change, PendingTCBStateChange::Exit) || state.state == TCBState::Exited {
            return false;
        }
        if !matches!(state.pending_state_change, PendingTCBStateChange::Stop(_)) {
            state.pending_state_change = PendingTCBStateChange::Stop(signum);
        }
        state.state == TCBState::Blocked
    }

    pub fn resume_from_stopped(&self) -> Option<WakeupAction> {
        let mut state = self.state.lock();
        if matches!(state.pending_state_change, PendingTCBStateChange::Stop(_)) {
            state.pending_state_change = PendingTCBStateChange::None;
        }
        if state.state != TCBState::Stopped {
            return None;
        }
        Some(state.make_ready())
    }

    /// Called by the task itself to apply pending state changes.
    /// Returns true if a state change was applied, false otherwise.
    pub fn apply_pending_state_change(&self) -> bool {
        // Decide and, where possible, commit the state transition under the state
        // lock. External notifications and potentially sleeping cleanup run only
        // after the lock guard has been dropped.
        let change = {
            let mut state = self.state.lock();
            match state.pending_state_change {
                PendingTCBStateChange::Exit => Some(PendingTCBStateChange::Exit),
                PendingTCBStateChange::Stop(signum) => match state.state {
                    TCBState::Ready | TCBState::Running => {
                        state.set_state(TCBState::Stopped);
                        state.wake_pending = false;
                        Some(PendingTCBStateChange::Stop(signum))
                    }
                    _ => None,
                },
                PendingTCBStateChange::PtraceStop(stop) => match state.state {
                    TCBState::Ready | TCBState::Running | TCBState::Stopped => {
                        state.set_state(TCBState::PtraceStop(stop));
                        state.wake_pending = false;
                        Some(PendingTCBStateChange::PtraceStop(stop))
                    }
                    _ => None,
                },
                PendingTCBStateChange::None => None,
            }
        };

        match change {
            Some(PendingTCBStateChange::Stop(signum)) => self.parent().notify_stopped(signum),
            Some(PendingTCBStateChange::PtraceStop(stop)) => self.notify_ptrace_stopped(stop.signum),
            Some(PendingTCBStateChange::Exit) => {
                // Releasing the last address-space user may sleep, so keep this
                // task schedulable until cleanup has completed.
                self.addrspace.release_task_user();
                let mut state = self.state.lock();
                state.set_state(TCBState::Exited);
                state.wake_pending = false;
            }
            Some(PendingTCBStateChange::None) => unreachable!(),
            None => return false,
        }

        true
    }

    pub fn exit(&self, status: super::pcb::ExitStatus) {
        debug_assert!(
            current::tid() == self.tid,
            "current tid {} != self.tid {}",
            current::tid(),
            self.tid
        );

        let mut state = self.state.lock();
        state.pending_state_change = PendingTCBStateChange::Exit;

        drop(state);

        let tid_address_guard = self.tid_address.lock();
        let tid_address = *tid_address_guard;
        drop(tid_address_guard);

        if let Some(tid_address) = tid_address {
            if self
                .addrspace
                .with_translated_write(tid_address, core::mem::size_of::<Tid>(), |dst| {
                    dst.copy_from_slice(&0_i32.to_ne_bytes());
                })
                .is_ok()
            {
                if let Ok(frame) = self.addrspace.get_frame(tid_address) {
                    let mut futex_manager = futex::manager();
                    let _ =
                        futex_manager.wake(futex::FutexKey::shared(&frame, tid_address & arch::PGMASK), 1, u32::MAX);
                }
            }
        }

        if self.parent.pid() == self.tid {
            self.parent.exit(status);
        } else {
            self.parent.remove_task(self);
            manager::remove(self.tid);
        }
    }

    pub fn is_exited(&self) -> bool {
        let state = self.state.lock();
        state.state == TCBState::Exited
    }

    pub fn set_parent_waiting_vfork(&self, parent: Option<Arc<dyn Task>>) {
        *self.parent_waiting_vfork.lock() = parent;
    }

    pub fn wake_parent_waiting_vfork(&self) {
        if let Some(parent) = self.parent_waiting_vfork.lock().take() {
            scheduler::wakeup_task_uninterruptible(parent, Event::VFork);
        }
    }

    pub fn push_ucontext_syscall_retreg(&self, retval: Option<usize>) {
        self.ucontext_syscall_retreg_backup.set(retval);
    }

    pub fn pop_ucontext_syscall_retreg(&self) -> Option<usize> {
        self.ucontext_syscall_retreg_backup.get_mut().take()
    }
}

impl Task for TCB {
    fn tid(&self) -> Tid {
        self.tid
    }

    fn egid(&self) -> Uid {
        self.parent.egid()
    }

    fn euid(&self) -> Uid {
        self.parent.euid()
    }

    fn fsuid(&self) -> Uid {
        self.parent.fsuid()
    }

    fn fsgid(&self) -> Uid {
        self.parent.fsgid()
    }

    fn supplementary_gids(&self) -> Vec<Uid> {
        self.parent.supplementary_gids()
    }

    fn kcontext_ptr(&self) -> *mut KernelContext {
        self.kernel_context.get()
    }

    fn check_kernel_stack_overflow(&self, addr: usize) -> bool {
        self.kernel_stack.check_stack_overflow(addr)
    }

    fn tcb(&self) -> &TCB {
        self
    }

    fn pause_system_time(&self) {
        self.pause_user_task_system_time();
    }

    fn resume_system_time(&self) {
        self.resume_user_task_system_time();
    }

    fn run_if_ready(&self) -> bool {
        let mut state = self.state.lock();
        if state.state != TCBState::Ready {
            return false;
        }
        debug_assert!(!state.on_cpu);
        debug_assert!(!state.wake_pending);
        state.set_state(TCBState::Running);
        state.on_cpu = true;
        return true;
    }

    // PERF_DEBUG(scheduler-time): cpu_id/now_us and debug_block_* updates are
    // temporary blocked-time accounting.
    fn finish_switch(&self, cpu_id: usize, now_us: u64) -> bool {
        let mut state = self.state.lock();
        debug_assert!(state.on_cpu);
        state.on_cpu = false;

        if state.wake_pending {
            state.wake_pending = false;
            #[cfg(feature = "scheduler-block-reason-debug")]
            {
                state.debug_block_name = None;
            }
            if matches!(
                state.state,
                TCBState::Blocked | TCBState::BlockedUninterruptible | TCBState::Stopped | TCBState::PtraceStop(_)
            ) {
                state.set_state(TCBState::Ready);
                return true;
            }
        }

        if state.state == TCBState::Running {
            #[cfg(feature = "scheduler-block-reason-debug")]
            {
                state.debug_block_name = None;
            }
            state.set_state(TCBState::Ready);
            return true;
        }
        #[cfg(feature = "scheduler-block-reason-debug")]
        {
            if matches!(state.state, TCBState::Blocked | TCBState::BlockedUninterruptible) {
                debug_assert!(state.debug_block_stamp.is_none());
                state.debug_block_stamp = state
                    .debug_block_name
                    .take()
                    .map(|name| DebugBlockStamp::new(cpu_id, now_us, name));
            } else {
                state.debug_block_name = None;
            }
        }
        #[cfg(not(feature = "scheduler-block-reason-debug"))]
        let _ = (cpu_id, now_us);
        false
    }

    // PERF_DEBUG(scheduler-time): name is retained until switch-out.
    fn block(&self, name: &'static str) -> bool {
        debug_assert!(
            current::tid() == self.tid,
            "current tid {} != self.tid {}",
            current::tid(),
            self.tid
        );

        let mut state = self.state.lock();
        match state.state {
            TCBState::Ready | TCBState::Running => {}
            _ => return false,
        }
        debug_assert!(state.on_cpu);
        debug_assert!(!state.wake_pending);
        #[cfg(feature = "scheduler-block-reason-debug")]
        {
            state.debug_block_name = Some(name);
        }
        #[cfg(not(feature = "scheduler-block-reason-debug"))]
        let _ = name;
        state.set_state(TCBState::Blocked);
        true
    }

    // PERF_DEBUG(scheduler-time): name is retained until switch-out.
    fn block_uninterruptible(&self, name: &'static str) -> bool {
        debug_assert!(current::tid() == self.tid);

        let mut state = self.state.lock();
        match state.state {
            TCBState::Ready | TCBState::Running => {}
            _ => return false,
        }
        debug_assert!(state.on_cpu);
        debug_assert!(!state.wake_pending);
        #[cfg(feature = "scheduler-block-reason-debug")]
        {
            state.debug_block_name = Some(name);
        }
        #[cfg(not(feature = "scheduler-block-reason-debug"))]
        let _ = name;
        state.set_state(TCBState::BlockedUninterruptible);
        true
    }

    fn unblock(&self) {
        let mut state = self.state.lock();
        debug_assert!(state.on_cpu);
        debug_assert!(matches!(
            state.state,
            TCBState::Blocked | TCBState::BlockedUninterruptible
        ));
        #[cfg(feature = "scheduler-block-reason-debug")]
        {
            state.debug_block_name = None;
        }
        state.set_state(TCBState::Running);
        state.wake_pending = false;
        self.wakeup_event.lock().take();
    }

    fn wakeup(&self, event: Event) -> Result<WakeupAction, WakeupFailure> {
        let mut state = self.state.lock();
        if state.wake_pending {
            return Err(WakeupFailure::NotBlocked);
        }
        match state.state {
            TCBState::Blocked => {
                *self.wakeup_event.lock() = Some(event);
                Ok(state.make_ready())
            }
            TCBState::BlockedUninterruptible => Err(WakeupFailure::BlockedUninterruptible),
            _ => Err(WakeupFailure::NotBlocked),
        }
    }

    fn wakeup_uninterruptible(&self, event: Event) -> Option<WakeupAction> {
        let mut state = self.state.lock();
        if state.wake_pending || !matches!(state.state, TCBState::Blocked | TCBState::BlockedUninterruptible) {
            crate::kwarn!(
                "Failed to wakeup_uninterruptible task {}, state is not blocked: {:?}",
                self.tid,
                state.state
            );
            return None;
        }
        *self.wakeup_event.lock() = Some(event);
        Some(state.make_ready())
    }

    fn take_wakeup_event(&self) -> Option<Event> {
        self.wakeup_event.lock().take()
    }

    #[cfg(feature = "lockdep")]
    fn lockstate(&self) -> &crate::klib::ksync::LockState {
        &self.lockstate
    }
}

unsafe impl Send for TCB {}
unsafe impl Sync for TCB {}
