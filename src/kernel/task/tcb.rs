use core::time::Duration;
use alloc::sync::Arc;
use alloc::vec;
use spin::Mutex;

use crate::kernel::config::UTASK_KSTACK_PAGE_COUNT;
use crate::kernel::mm::uptr::UPtr;
use crate::kernel::scheduler::current;
use crate::kernel::scheduler::Task;
use crate::kernel::usync::futex::{self, RobustListHead};
use crate::kernel::config;
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::tid::Tid;
use crate::kernel::task::PCB;
use crate::kernel::task::fdtable::{FDFlags, FDTable};
use crate::kernel::mm::{AddrSpace, elf};
use crate::kernel::mm::maparea::{AuxKey, Auxv};
use crate::kernel::event::{Event, timer};
use crate::kernel::ipc::{PendingSignal, SignalSet};
use crate::kernel::errno::Errno;
use crate::fs::file::{File, FileFlags, CharFile};
use crate::fs::{Perm, PermFlags, vfs};
use crate::kinfo;
use crate::klib::SpinLock;
use crate::arch::{UserContext, KernelContext, UserContextTrait};
use crate::arch;
use crate::driver;
use crate::ktrace;

use super::kernelstack::KernelStack;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Ready,
    Blocked,
    BlockedUninterruptible,
    Exited,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskStateSet {
    pub state: TaskState,
    
    pub pending_signal: Option<PendingSignal>,
    pub signal_to_wait: SignalSet,
}

impl TaskStateSet {
    pub fn new() -> Self {
        Self {
            state: TaskState::Ready,
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
    pub tid: Tid,
    pub parent: Arc<PCB>,
    tid_address: Mutex<Option<usize>>,
    pub robust_list: SpinLock<Option<UPtr<RobustListHead>>>,
    
    user_context_ptr: *mut UserContext,
    user_context_uaddr: usize,
    kernel_context: KernelContext,
    pub kernel_stack: KernelStack,

    addrspace: Arc<AddrSpace>,
    fdtable: Arc<SpinLock<FDTable>>,

    pub signal_mask: SpinLock<SignalSet>,

    state: SpinLock<TaskStateSet>,
    pub wakeup_event: SpinLock<Option<Event>>,
    parent_waiting_vfork: SpinLock<Option<Arc<dyn Task>>>,
    pub time_counter: SpinLock<TimeCounter>,
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
            parent: parent.clone(),
            tid_address: Mutex::new(None),
            robust_list: SpinLock::new(None),
            
            user_context_ptr,
            user_context_uaddr,
            kernel_context: KernelContext::new(&kernel_stack),
            kernel_stack,

            addrspace,
            fdtable,

            signal_mask: SpinLock::new(SignalSet::empty()),
            
            state: SpinLock::new(TaskStateSet::new()),
            wakeup_event: SpinLock::new(None),
            parent_waiting_vfork: SpinLock::new(None),
            time_counter: SpinLock::new(TimeCounter::new()),
        });

        tcb
    }

    pub fn new_inittask(
        tid: i32, 
        parent: &Arc<PCB>,
        file: File,
        argv: &[&str],
        envp: &[&str],
    ) -> Arc<Self> {
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

                let interpreter_file = vfs::open_file(
                    interpreter, 
                    FileFlags::dontcare(),
                    &Perm::new(PermFlags::X)
                ).expect("Failed to open.");
                return Self::new_inittask(tid, parent, interpreter_file, &new_argv, envp);
            }
        }

        let file = Arc::new(file);
        
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

        let mut fdtable = FDTable::new();
        let stdout_dev = driver::get_char_driver("sbi-console").unwrap();
        for _ in 0..3 {
            fdtable.push(Arc::new(CharFile::new(stdout_dev.clone())), FDFlags::empty()).unwrap();
        }
        fdtable.push(file.clone(), FDFlags::empty()).unwrap();

        let mut user_context = UserContext::new();
        user_context.set_user_stack_top(userstack_top);
        user_context.set_user_entry(user_entry);
        
        let tcb = Self::new(
            tid, 
            parent,
            user_context, 
            Arc::new(addrspace),
            Arc::new(SpinLock::new(fdtable))
        );
        
        tcb
    }

    pub fn new_clone(
        &self,
        tid: Tid,
        parent: &Arc<PCB>,
        userstack: usize,
        flags: &TaskCloneFlags,
        tls: Option<usize>,
    ) -> Arc<Self> {
        let mut new_user_context = UserContext::new();
        self.with_user_context(|user_context  | {
            new_user_context = user_context.new_clone();
        });

        if flags.vm {
            new_user_context.set_user_stack_top(userstack);
        }

        let new_addrspace;

        if flags.vm {
            new_addrspace = self.addrspace.clone();
        } else {
            let addrspace = self.addrspace.fork();
            new_user_context.set_addrspace(&addrspace);
            new_addrspace = Arc::new(addrspace);
        }

        new_user_context.skip_syscall_instruction();

        if let Some(tls) = tls {
            new_user_context.set_tls(tls);
        }

        let new_tcb = Self::new(
            tid,
            parent,
            new_user_context,
            new_addrspace,
            Arc::new(SpinLock::new(self.fdtable.lock().fork())),
        );

        new_tcb
    }

    pub fn new_exec(
        &self,
        file: File,
        argv: &[&str],
        envp: &[&str],
    ) -> Result<Arc<Self>, Errno> {
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
                )?;
                return self.new_exec(interpreter_file, &new_argv, envp);
            }
        }

        let file = Arc::new(file);
        
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
            Arc::new(addrspace),
            self.fdtable().clone(),
        );

        Ok(new_tcb)
    }

    pub fn get_tid(&self) -> i32 {
        self.tid
    }

    pub fn get_user_context_uaddr(&self) -> usize {
        self.user_context_uaddr
    }

    pub fn get_user_context_ptr(&self) -> *mut UserContext {
        self.user_context_ptr
    }

    pub fn get_parent(&self) -> &Arc<PCB> {
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

    pub fn set_tid_address(&self, addr: usize) {
        *self.tid_address.lock() = Some(addr);
    }

    // pub fn wakeup(self: &Arc<Self>, event: Event) {
    //     let mut state = self.state().lock();
    //     if state.state != TaskState::Blocked {
    //         return;
    //     }
    //     state.state = TaskState::Ready;
    //     *self.wakeup_event.lock() = Some(event);
        
    //     scheduler::push_task(Task::User(self.clone()));
    // }

    // pub fn wakeup_uninterruptible(self: &Arc<Self>, event: Event) {
    //     let mut state = self.state().lock();
    //     match state.state {
    //         TaskState::Blocked | TaskState::BlockedUninterruptible => {},
    //         _ => return,
    //     }
    //     state.state = TaskState::Ready;
    //     *self.wakeup_event.lock() = Some(event);
        
    //     scheduler::push_task(Task::User(self.clone()));
    // }

    // pub fn take_wakeup_event(&self) -> Option<Event> {
    //     self.wakeup_event.lock().take()
    // }

    // pub fn run(&self) {
    //     let mut state = self.state.lock();
    //     // assert!(state.state == TaskState::Ready);
    //     state.state = TaskState::Running;
    // }

    pub fn state(&self) -> &SpinLock<TaskStateSet> {
        &self.state
    }

    pub fn with_state_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut TaskStateSet) -> R,
    {
        let mut state = self.state.lock();
        f(&mut state)
    }

    pub fn exit(&self, code: u8) {
        let mut state = self.state.lock();

        if let Some(tid_address) = *self.tid_address.lock() {
            if let Ok(tid_kaddr) = self.addrspace.translate_write(tid_address) {
                // debug_assert!(tid_kaddr & 0x3 == 0);
                unsafe { *(tid_kaddr as *mut Tid) = 0 };
                let _ = futex::wake(tid_kaddr, 1, u32::MAX);
            }
        }

        state.state = TaskState::Exited;

        drop(state);

        if self.parent.get_pid() == self.tid {
            self.parent.exit(code);
        }
    }

    pub fn get_kernel_stack_top(&self) -> usize {
        self.kernel_stack.get_top()
    }

    pub fn set_parent_waiting_vfork(&self, parent: Option<Arc<dyn Task>>) {
        *self.parent_waiting_vfork.lock() = parent;
    }

    pub fn wake_parent_waiting_vfork(&self) {
        if let Some(parent) = self.parent_waiting_vfork.lock().take() {
            parent.wakeup_uninterruptible(Event::VFork);
        }
    }
}

impl Drop for TCB {
    fn drop(&mut self) {
        // kinfo!("Dropping TCB of task {}", self.tid);
    }
}

impl Task for TCB {
    fn tid(&self) -> Tid {
        self.tid
    }

    fn get_kcontext_ptr(&self) -> *mut KernelContext {
        &self.kernel_context as *const KernelContext as *mut KernelContext
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
        debug_assert!(current::tid() == self.tid);
        
        let mut state = self.state.lock();
        match state.state {
            TaskState::Ready | TaskState::Running => {},
            _ => return false,
        }
        state.state = TaskState::Blocked;
        // kinfo!("Task {} blocked: {}", self.tid, _reason);
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

    fn wakeup(&self, event: Event) -> bool {
        let mut state = self.state().lock();
        if state.state != TaskState::Blocked {
            return false;
        }
        state.state = TaskState::Ready;
        *self.wakeup_event.lock() = Some(event);
        true
    }

    fn wakeup_uninterruptible(&self, event: Event) {
        let mut state = self.state().lock();
        match state.state {
            TaskState::Blocked | TaskState::BlockedUninterruptible => {},
            _ => return,
        }
        state.state = TaskState::Ready;
        *self.wakeup_event.lock() = Some(event);
    }

    fn take_wakeup_event(&self) -> Option<Event> {
        self.wakeup_event.lock().take()
    }
}

unsafe impl Send for TCB {}
unsafe impl Sync for TCB {}
