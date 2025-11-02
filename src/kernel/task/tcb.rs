use alloc::sync::Arc;
use alloc::vec;
use spin::Mutex;

use crate::kernel::scheduler::current;
use crate::kernel::{config, scheduler};
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::tid::Tid;
use crate::kernel::task::PCB;
use crate::kernel::task::fdtable::{FDFlags, FDTable};
use crate::kernel::mm::{AddrSpace, elf};
use crate::kernel::mm::maparea::{AuxKey, Auxv};
use crate::kernel::event::Event;
use crate::kernel::ipc::{PendingSignal, SignalSet};
use crate::kernel::errno::Errno;
use crate::fs::file::{File, FileFlags, CharFile};
use crate::fs::vfs;
use crate::klib::SpinLock;
use crate::arch::{UserContext, KernelContext, UserContextTrait};
use crate::{arch, lock_debug};
use crate::driver;
use crate::{kdebug, kinfo, ktrace};

use super::kernelstack::KernelStack;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Ready,
    Blocked,
    Exited,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskStateSet {
    pub state: TaskState,
    pub event: Option<Event>,
    pub exit_code: u8,
    
    pub pending_signal: Option<PendingSignal>,
    pub waiting_signal: SignalSet
}

impl TaskStateSet {
    pub fn new() -> Self {
        Self {
            state: TaskState::Ready,
            event: None,
            exit_code: 0,
            pending_signal: None,
            waiting_signal: SignalSet::empty()
        }
    }
}

pub struct TCB {
    pub tid: Tid,
    pub parent: Arc<PCB>,
    tid_address: Mutex<Option<usize>>,
    
    user_context_ptr: *mut UserContext,
    user_context_uaddr: usize,
    kernel_context: KernelContext,
    kernel_stack: KernelStack,

    addrspace: Arc<AddrSpace>,
    fdtable: Arc<SpinLock<FDTable>>,

    pub signal_mask: SpinLock<SignalSet>,

    state: SpinLock<TaskStateSet>,
}

impl TCB {
    pub fn new(
        tid: i32, 
        parent: &Arc<PCB>, 
        
        mut user_context: UserContext,
        
        addrspace: Arc<AddrSpace>,
        fdtable: Arc<SpinLock<FDTable>>,
    ) -> Arc<Self> {
        let kernel_stack = KernelStack::new(); 
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
            
            user_context_ptr,
            user_context_uaddr,
            kernel_context: KernelContext::new(&kernel_stack),
            kernel_stack,

            addrspace,
            fdtable: fdtable,

            signal_mask: SpinLock::new(SignalSet::empty()),
            
            state: SpinLock::new(TaskStateSet::new()),
        });

        tcb
    }

    pub fn new_inittask(
        tid: i32, 
        parent: &Arc<PCB>,
        file: &Arc<File>,
        argv: &[&str],
        envp: &[&str],
    ) -> Arc<Self> {
        let mut addrspace = AddrSpace::new();
        let (user_entry, dyn_info) = elf::loader::load_elf(file, &mut addrspace)
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
        self: &Arc<Self>,
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

        if flags.thread {
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
        self: &Arc<Self>,
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

                let interpreter_file = vfs::open_file(interpreter, FileFlags::dontcare())?;
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

    /// Returns a mutable pointer to the kernel context
    pub fn get_kernel_context_ptr(&self) -> *mut KernelContext {
        &self.kernel_context as *const KernelContext as *mut KernelContext
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

    pub fn block(self: &Arc<Self>, _reason: &str) -> bool {
        assert!(Arc::ptr_eq(self, current::tcb()));
        
        let mut state = self.state.lock();
        match state.state {
            TaskState::Ready | TaskState::Running => {},
            _ => return false,
        }
        state.state = TaskState::Blocked;
        true
    }

    pub fn wakeup(self: &Arc<Self>, event: Event) {
        let mut state = lock_debug!(self.state());
        if state.state != TaskState::Blocked {
            return;
        }
        state.state = TaskState::Ready;
        state.event = Some(event);
        
        scheduler::push_task(self.clone());
    }

    pub fn run(&self) {
        let mut state = self.state.lock();
        // assert!(state.state == TaskState::Ready);
        state.state = TaskState::Running;
    }

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
        if state.state == TaskState::Exited {
            kdebug!("Task {} already exited", self.tid);
            return;
        }

        // let tid_address = self.tid_address.lock().take();
        // if let Some(tid_address) = tid_address {
        //     self.addrspace.copy_to_user(tid_address, &(0 as Tid).to_le_bytes()).expect("Failed to clear TID address");
        // }

        // kinfo!("Task {} exited with code {}", self.tid, code);

        state.exit_code = code;
        state.state = TaskState::Exited;

        drop(state);

        if self.parent.get_pid() == self.tid {
            self.parent.exit(code);
        }
    }

    pub fn get_kernel_stack_top(&self) -> usize {
        self.kernel_stack.get_top()
    }
}

impl Drop for TCB {
    fn drop(&mut self) {
        // kinfo!("Dropping TCB of task {}", self.tid);
    }
}

unsafe impl Send for TCB {}
unsafe impl Sync for TCB {}
