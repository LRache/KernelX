use alloc::sync::{Arc, Weak};
use spin::{RwLock, Mutex};
use core::cell::UnsafeCell;

use crate::kernel::config;
use crate::kernel::task::def::TaskCloneFlags;
use crate::kernel::task::tid::Tid;
use crate::kernel::task::PCB;
use crate::kernel::task::fdtable::FDTable;
use crate::kernel::mm::{AddrSpace, elf};
use crate::kernel::mm::maparea::{AuxKey, Auxv};
use crate::kernel::errno::Errno;
use crate::fs::file::{File, FileFlags};
use crate::fs::vfs;
use crate::fs::InodeWrapper;
use crate::arch::{UserContext, KernelContext};
use crate::{arch, kdebug};
use crate::ktrace;
use super::kernelstack::KernelStack;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Running,
    Ready,
    // Blocked,
    Exited,
}

pub struct TCB {
    pub tid: Tid,
    pub parent: Weak<PCB>,
    exit_code: Mutex<u8>,
    tid_address: Mutex<Option<usize>>,
    
    user_context_ptr: *mut UserContext,
    user_context_uaddr: usize,
    user_entry: UnsafeCell<usize>,
    kernel_context: KernelContext,
    _kernel_stack: KernelStack,

    addrspace: Arc<AddrSpace>,
    fdtable: Arc<FDTable>,
    pwd: Arc<InodeWrapper>,

    state: RwLock<ThreadState>,
}

impl TCB {
    pub fn new(
        tid: i32, 
        parent: &Arc<PCB>, 
        
        mut user_context: UserContext,
        user_entry: usize,
        
        addrspace: Arc<AddrSpace>,
        fdtable: Arc<FDTable>,

        pwd: &Arc<InodeWrapper>,
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
            parent: Arc::downgrade(&parent),
            exit_code: Mutex::new(0),
            tid_address: Mutex::new(None),
            
            user_context_ptr,
            user_context_uaddr,
            user_entry: UnsafeCell::new(user_entry),
            kernel_context: KernelContext::new(&kernel_stack),
            _kernel_stack: kernel_stack,

            addrspace,
            fdtable,
            pwd: pwd.clone(),
            state: RwLock::new(ThreadState::Ready),
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
            auxv.push(AuxKey::PAGESZ, arch::PGSIZE);
            auxv.push(AuxKey::FLAGS, 0);
            auxv.push(AuxKey::ENTRY, dyn_info.user_entry);
            auxv.push(AuxKey::RANDOM, config::USER_RANDOM_ADDR_BASE);
            ktrace!("Dynamic linker info: {:?}", dyn_info);
        }

        let userstack_top = addrspace.create_user_stack(argv, envp, &auxv).expect("Failed to push args and envp to userstack");

        let fdtable = FDTable::new();
        for _ in 0..3 {
            fdtable.push(vfs::stdout::stdout()).expect("Failed to push stdout to FDTable");
        }

        let mut user_context = UserContext::new();
        user_context.set_user_stack_top(userstack_top);
        
        let tcb = Self::new(
            tid, 
            parent, 
            user_context, 
            user_entry, 
            Arc::new(addrspace),
            Arc::new(fdtable),
            vfs::open("/", FileFlags::dontcare()).unwrap().get_inode()
        );
        
        tcb
    }

    pub fn new_clone(
        self: &Arc<Self>,
        tid: Tid,
        parent: &Arc<PCB>,
        userstack: usize,
        flags: &TaskCloneFlags
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

        let new_tcb = Self::new(
            tid,
            parent,
            new_user_context,
            self.get_user_entry(),
            new_addrspace,
            Arc::new(self.fdtable.fork()),
            &self.pwd,
        );
        
        new_tcb.set_state(ThreadState::Ready);

        new_tcb
    }

    pub fn new_exec(
        self: &Arc<Self>,
        file: File,
        argv: &[&str],
        envp: &[&str],
    ) -> Result<Arc<Self>, Errno> {
        let file = Arc::new(file);
        
        let mut addrspace = AddrSpace::new();
        let (user_entry, dyn_info) = elf::loader::load_elf(&file, &mut addrspace)?;

        let mut auxv = Auxv::new();
        if let Some(dyn_info) = dyn_info {
            auxv.push(AuxKey::BASE, dyn_info.interpreter_base);
            auxv.push(AuxKey::PHDR, dyn_info.phdr_addr);
            auxv.push(AuxKey::PHENT, dyn_info.phent as usize);
            auxv.push(AuxKey::PHNUM, dyn_info.phnum as usize);
            auxv.push(AuxKey::PAGESZ, arch::PGSIZE);
            auxv.push(AuxKey::ENTRY, dyn_info.user_entry);
            auxv.push(AuxKey::RANDOM, config::USER_RANDOM_ADDR_BASE);
        }

        let usetstack_top = addrspace.create_user_stack(argv, envp, &auxv)?;

        let mut new_user_context = UserContext::new();
        new_user_context.set_user_stack_top(usetstack_top);

        let new_tcb = TCB::new(
            self.tid,
            &self.parent.upgrade().expect("Parent PCB should exist"),
            new_user_context,
            user_entry,
            Arc::new(addrspace),
            self.fdtable.clone(),
            &self.pwd,
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

    pub fn get_pwd(&self) -> &Arc<InodeWrapper> {
        &self.pwd
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

    pub fn get_addrspace(&self) -> &Arc<AddrSpace> {
        &self.addrspace
    }

    pub fn get_fd_table(&self) -> &FDTable {
        &self.fdtable
    }

    pub fn get_user_entry(&self) -> usize {
        unsafe { *self.user_entry.get() }
    }

    pub fn set_user_entry(&self, entry: usize) {
        unsafe { self.user_entry.get().write(entry); }
    }

    pub fn set_tid_address(&self, addr: usize) {
        *self.tid_address.lock() = Some(addr);
    }

    pub fn get_state(&self) -> ThreadState {
        *self.state.read()
    }
    
    pub fn set_state(&self, new_state: ThreadState) {
        *self.state.write() = new_state;
    }

    pub fn get_exit_code(&self) -> u8 {
        *self.exit_code.lock()
    }

    pub fn exit(&self, code: u8) {
        if *self.state.read() == ThreadState::Exited {
            return;
        }

        // let tid_address = self.tid_address.lock().take();
        // if let Some(tid_address) = tid_address {
        //     self.addrspace.copy_to_user(tid_address, &(0 as Tid).to_le_bytes()).expect("Failed to clear TID address");
        // }

        kdebug!("Task {} exited with code {}", self.tid, code);

        *self.exit_code.lock() = code;
        self.set_state(ThreadState::Exited);

        let parent = self.parent.upgrade().expect("Parent PCB should exist");
        if parent.get_pid() == self.tid {
            parent.exit(code);
        }
    }
}

unsafe impl Send for TCB {}
unsafe impl Sync for TCB {}
