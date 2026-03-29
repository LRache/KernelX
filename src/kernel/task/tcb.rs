use core::time::Duration;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use core::cell::UnsafeCell;

use crate::driver::chosen::kclock;
use crate::fs::file::FileOps;
use crate::kernel::config::UTASK_KSTACK_PAGE_COUNT;
use crate::kernel::errno::SysResult;
use crate::kernel::scheduler;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::Task;
use crate::kernel::usync::futex;
use crate::kernel::config;
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::PCB;
use crate::kernel::task::fdtable::{FDFlags, FDTable};
use crate::kernel::mm::{AddrSpace, elf};
use crate::kernel::mm::maparea::{AuxKey, Auxv};
use crate::kernel::event::{Event, timer};
use crate::kernel::ipc::{PendingSignal, SignalSet};
use crate::kernel::errno::Errno;
use crate::kernel::scheduler::{TaskState, Tid, KernelStack};
use crate::fs::file::{File, FileFlags};
use crate::fs::{Perm, PermFlags, vfs};
use crate::kinfo;
use crate::klib::SpinLock;
use crate::arch::{UserContext, KernelContext, UserContextTrait};
use crate::arch;
use crate::ktrace;
#[derive(Debug, Clone, Copy)]
pub struct TaskStateSet {
    state: TaskState,
    dead: bool,
    
    pub pending_signal: Option<PendingSignal>,
    pub signal_to_wait: SignalSet,
}

impl Default for TaskStateSet {
    fn default() -> Self {
        Self {
            state: TaskState::Ready,
            dead: false,
            pending_signal: None,
            signal_to_wait: SignalSet::empty()
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
}

pub struct TCB {
    tid: Tid,
    create_time: Duration,
    parent: Arc<PCB>,
    tid_address: SpinLock<Option<usize>>,
    pub robust_list: SpinLock<Option<usize>>,
    
    user_context_ptr: *mut UserContext,
    user_context_uaddr: usize,
    kernel_context: UnsafeCell<KernelContext>,
    pub kernel_stack: KernelStack,

    addrspace: Arc<AddrSpace>,
    fdtable: Arc<SpinLock<FDTable>>,

    pub signal_mask: SpinLock<SignalSet>,

    state: SpinLock<TaskStateSet>,
    pub wakeup_event: SpinLock<Option<Event>>,
    parent_waiting_vfork: SpinLock<Option<Arc<dyn Task>>>,
    pub time_counter: SpinLock<TimeCounter>,

    #[cfg(feature = "deadlock-detect")]
    lockstate: crate::klib::ksync::LockState,
}

impl TCB {
    pub fn new(
        tid: i32, 
        parent: &Arc<PCB>, 
        
        mut user_context: UserContext,
        
        addrspace: Arc<AddrSpace>,
        fdtable: Arc<SpinLock<FDTable>>,
    ) -> Arc<Self> {
        let kernel_stack = KernelStack::new(UTASK_KSTACK_PAGE_COUNT); 
        user_context.set_kernel_stack_top(kernel_stack.get_top());

        let (user_context_uaddr, user_context_ptr) = addrspace.alloc_usercontext_page();
        user_context.set_addrspace(&addrspace);

        unsafe {
            user_context_ptr.write(user_context);
        }

        let tcb = Arc::new(Self {
            tid,
            create_time: kclock::now().unwrap_or(Duration::ZERO),
            parent: parent.clone(),
            tid_address: SpinLock::new(None, "TCB::tid_address"),
            robust_list: SpinLock::new(None, "TCB::robust_list"),

            user_context_ptr,
            user_context_uaddr,
            kernel_context: UnsafeCell::new(KernelContext::new(&kernel_stack)),
            kernel_stack,

            addrspace,
            fdtable,

            signal_mask: SpinLock::new(SignalSet::empty(), "TCB::signal_mask"),

            state: SpinLock::new(TaskStateSet::default(), "TCB::state"),
            wakeup_event: SpinLock::new(None, "TCB::wakeup_event"),
            parent_waiting_vfork: SpinLock::new(None, "TCB::parent_waiting_vfork"),
            time_counter: SpinLock::new(TimeCounter::new(), "TCB::time_counter"),
            #[cfg(feature = "deadlock-detect")]
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
        tty: &str
    ) -> (Arc<Self>, String) {
        let file = vfs::open_file(
            initpath, 
            FileFlags::dontcare(),
            &Perm::new(PermFlags::R | PermFlags::X)
        )
        .expect("Failed to open init file")
        .downcast_arc::<File>().map_err(|_| Errno::ENOEXEC)
        .expect("Failed to open init file as File");
        
        // Read the shebang
        let mut first_line = [0u8; 128];
        let n = file.read_at(&mut first_line, 0).expect("Failed to read first line of init file");
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
                for arg in argv {
                    new_argv.push(arg);
                }
                new_argv.push(initpath);

                return Self::new_inittask(tid, parent, interpreter, &new_argv, envp, tty);
            }
        }

        let exec_path = file.get_dentry().unwrap().get_path();
        
        let mut addrspace = AddrSpace::new();
        let (user_entry, dyn_info) = elf::loader::load_elf(&file, &mut addrspace)
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

        let userstack_top = addrspace.create_user_stack(argv, envp, &auxv).expect("Failed to push args and envp to userstack");

        let tty = vfs::open_file(
            tty, 
            FileFlags { readable: true, writable: true, blocked: true },
            &Perm::new(PermFlags::R | PermFlags::W)
        ).expect("Failed to open tty for init task");

        let mut fdtable = FDTable::new();
        for _ in 0..3 {
            fdtable.push(tty.clone(), FDFlags::empty()).unwrap();
        }
        fdtable.push(file.clone(), FDFlags::empty()).unwrap();

        let mut user_context = UserContext::new();
        user_context.set_user_stack_top(userstack_top);
        user_context.set_user_entry(user_entry);
        
        let tcb = Self::new(
            tid, 
            parent,
            user_context, 
            addrspace,
            Arc::new(SpinLock::new(fdtable, "TCB::fdtable"))
        );
        
        (tcb, exec_path)
    }

    pub fn new_clone(
        &self,
        tid: Tid,
        parent: &Arc<PCB>,
        userstack: usize,
        flags: &TaskCloneFlags,
        tls: Option<usize>,
    ) -> Arc<Self> {
        let mut new_user_context = self.with_user_context(|user_context| {
            user_context.new_clone()
        });

        let new_addrspace = if flags.vm {
            new_user_context.set_user_stack_top(userstack);
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
            self.fdtable.clone()
        } else {
            Arc::new(SpinLock::new(self.fdtable.lock().fork(), "TCB::fdtable"))
        };

        let new_tcb = Self::new(
            tid,
            parent,
            new_user_context,
            new_addrspace,
            new_fdtable,
        );

        new_tcb
    }

    pub fn new_exec(
        &self,
        file: Arc<File>,
        argv: &[&str],
        envp: &[&str],
    ) -> SysResult<(Arc<Self>, String)> {
        // Read the shebang
        let mut first_line = [0u8; 128];
        let n = file.read_at(&mut first_line, 0)?;
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
                for arg in argv {
                    new_argv.push(arg);
                }

                let interpreter_file = vfs::open_file(
                    interpreter, 
                    FileFlags::dontcare(),
                    &Perm::new(PermFlags::X)
                )?.downcast_arc::<File>().map_err(|_| Errno::ENOEXEC)?;
                return self.new_exec(interpreter_file, &new_argv, envp);
            }
        }

        // SAFETY: File MUTS HAVE dentry and path.
        let exec_path = file.get_dentry().unwrap().get_path();
        
        let mut addrspace = AddrSpace::new();
        let (user_entry, dyn_info) = elf::loader::load_elf(&file, &mut addrspace)?;

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

        let usetstack_top = addrspace.create_user_stack(argv, envp, &auxv)?;

        let mut new_user_context = UserContext::new();
        new_user_context.set_user_stack_top(usetstack_top);
        new_user_context.set_user_entry(user_entry);

        self.fdtable().lock().cloexec();

        let new_tcb = TCB::new(
            self.tid,
            &self.parent,
            new_user_context,
            addrspace,
            self.fdtable().clone(),
        );

        Ok((new_tcb, exec_path))
    }

    pub fn tid(&self) -> i32 {
        self.tid
    }

    pub fn get_user_context_uaddr(&self) -> usize {
        self.user_context_uaddr
    }

    pub fn get_user_context_ptr(&self) -> *mut UserContext {
        self.user_context_ptr
    }

    pub fn parent(&self) -> &Arc<PCB> {
        &self.parent
    }

    pub fn with_user_context<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&UserContext) -> R,
    {
        let user_context = unsafe { self.user_context_ptr.as_ref().unwrap() };
        f(user_context)
    }

    pub fn with_user_context_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut UserContext) -> R,
    {
        let user_context = unsafe { self.user_context_ptr.as_mut().unwrap() };
        f(user_context)
    }

    pub fn user_context(&self) -> &mut UserContext {
        unsafe { self.user_context_ptr.as_mut().unwrap() }
    }

    pub fn get_addrspace(&self) -> &Arc<AddrSpace> {
        &self.addrspace
    }

    pub fn fdtable(&self) -> &Arc<SpinLock<FDTable>> {
        &self.fdtable
    }

    pub fn get_signal_mask(&self) -> SignalSet {
        *self.signal_mask.lock()
    }

    pub fn set_signal_mask(&self, mask: SignalSet) {
        *self.signal_mask.lock() = mask;
    }

    pub fn swap_signal_mask(&self, mask: SignalSet) -> SignalSet {
        let mut signal_mask = self.signal_mask.lock();
        let old_mask = *signal_mask;
        *signal_mask = mask;
        old_mask
    }

    pub fn set_tid_address(&self, addr: usize) {
        *self.tid_address.lock() = Some(addr);
    }

    pub fn state(&self) -> &SpinLock<TaskStateSet> {
        &self.state
    }

    pub fn create_time(&self) -> Duration {
        self.create_time
    }

    // pub fn with_state_mut<F, R>(&self, f: F) -> R
    // where
    //     F: FnOnce(&mut TaskStateSet) -> R,
    // {
    //     let mut state = self.state.lock();
    //     f(&mut state)
    // }

    pub fn set_dead(&self) {
        let mut state = self.state.lock();
        state.dead = true;
    }

    pub fn state_dead_to_exited(&self) -> bool {
        let mut state = self.state.lock();
        if state.dead {
            state.state = TaskState::Exited;
            true
        } else {
            false
        }
    }

    pub fn exit(&self, code: u8) {
        debug_assert!(current::tid() == self.tid, "current tid {} != self.tid {}", current::tid(), self.tid);
        
        let mut state = self.state.lock();
        state.dead = true;

        drop(state);

        let tid_address_guard = self.tid_address.lock();
        let tid_address = *tid_address_guard;
        drop(tid_address_guard);

        if let Some(tid_address) = tid_address {
            if let Ok(tid_kaddr) = self.addrspace.translate_write(tid_address) {
                // debug_assert!(tid_kaddr & 0x3 == 0);
                unsafe { *(tid_kaddr as *mut Tid) = 0 };
                let _ = futex::wake(tid_kaddr, 1, u32::MAX);
            }
        }

        if self.parent.pid() == self.tid {
            self.parent.exit(code);
        }
        
        // cleanup addrspace before scheduler
        if Arc::strong_count(&self.addrspace) == 1 {
            self.addrspace.cleanup();
        }
    }

    pub fn is_exited(&self) -> bool {
        let state = self.state.lock();
        state.state == TaskState::Exited
    }

    pub fn set_parent_waiting_vfork(&self, parent: Option<Arc<dyn Task>>) {
        *self.parent_waiting_vfork.lock() = parent;
    }

    pub fn wake_parent_waiting_vfork(&self) {
        if let Some(parent) = self.parent_waiting_vfork.lock().take() {
            // kinfo!("Waking up parent task {} waiting for vfork", parent.tid());
            scheduler::wakeup_task_uninterruptible(parent, Event::VFork);
        }
    }
}

impl Task for TCB {
    fn tid(&self) -> Tid {
        self.tid
    }

    fn kcontext(&self) -> &mut KernelContext {
        unsafe { (self.kernel_context.get() as *mut KernelContext).as_mut() }.unwrap()
    }

    fn kstack(&self) -> &KernelStack {
        &self.kernel_stack
    }

    fn tcb(&self) -> &TCB {
        self
    }

    fn run_if_ready(&self) -> bool {
        let mut state = self.state.lock();
        if state.state != TaskState::Ready {
            return false;
        }
        state.state = TaskState::Running;
        return true;
    }

    fn state_running_to_ready(&self) -> bool {
        let mut state = self.state.lock();
        if state.state != TaskState::Running {
            return false;
        }
        state.state = TaskState::Ready;
        true
    }

    fn block(&self, _reason: &str) -> bool {
        debug_assert!(current::tid() == self.tid, "current tid {} != self.tid {}", current::tid(), self.tid);
        
        let mut state = self.state.lock();
        match state.state {
            TaskState::Ready | TaskState::Running => {},
            _ => return false,
        }
        state.state = TaskState::Blocked;
        true
    }

    fn block_uninterruptible(&self, _reason: &str) -> bool {
        debug_assert!(current::tid() == self.tid);
        
        let mut state = self.state.lock();
        match state.state {
            TaskState::Ready | TaskState::Running => {},
            _ => return false,
        }
        state.state = TaskState::BlockedUninterruptible;
        true
    }

    fn unblock(&self) {
        let mut state = self.state.lock();
        debug_assert!(state.state == TaskState::Blocked);
        state.state = TaskState::Running;
    }

    fn wakeup(&self, event: Event) -> bool {
        let mut state = self.state.lock();
        if state.state != TaskState::Blocked {
            return false;
        }
        state.state = TaskState::Ready;
        *self.wakeup_event.lock() = Some(event);
        true
    }

    fn wakeup_uninterruptible(&self, event: Event) -> bool {
        let mut state = self.state.lock();
        if !matches!(state.state, TaskState::Blocked | TaskState::BlockedUninterruptible) {
            crate::kwarn!("Failed to wakeup_uninterruptible task {}, state is not blocked: {:?}", self.tid, state.state);
            return false;
        }
        state.state = TaskState::Ready;
        *self.wakeup_event.lock() = Some(event);
        true
    }

    fn take_wakeup_event(&self) -> Option<Event> {
        self.wakeup_event.lock().take()
    }

    #[cfg(feature = "deadlock-detect")]
    fn lockstate(&self) -> &crate::klib::ksync::LockState {
        &self.lockstate
    }
}

unsafe impl Send for TCB {}
unsafe impl Sync for TCB {}

impl Drop for TCB {
    fn drop(&mut self) {
        kinfo!("Dropping TCB with TID {}", self.tid);
    }
}
