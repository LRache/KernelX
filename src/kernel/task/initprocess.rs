use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

use alloc::sync::Arc;

use crate::kernel::task::pcb::PCB;
use crate::fs::vfs;
use crate::fs::file::FileFlags;

const INITPATH: &'static str = match option_env!("KERNELX_INITPATH") {
    Some(path) => path,
    None => "/glibc/basic/brk",
};

const INITPWD: &'static str = match option_env!("KERNELX_INITPWD") {
    Some(path) => path,
    None => "/",
};

const INIT_ARGV: &[&str] = &[INITPATH];
const INIT_ENVP: &[&str] = &[
    // "LD_LIBRARY_PATH=/lib", 
    // "LD_SHOW_AUXV=1", 
    // "LD_DEBUG=all", 
    // "LD_BIND_NOW=1", 
    // "LD_PRELOAD=",
    // "LD_USE_LOAD_BIAS=0"
];

struct InitProcess(UnsafeCell<MaybeUninit<Arc<PCB>>>);

unsafe impl Sync for InitProcess {}

static INIT_PROCESS: InitProcess = InitProcess(UnsafeCell::new(MaybeUninit::uninit()));

pub fn create_initprocess() {
    let initfile = vfs::open_file(INITPATH, FileFlags::dontcare())
                                    .expect("Failed to open init file");
    
    let pcb = PCB::new_initprocess(
        initfile, 
        INITPWD, 
        INIT_ARGV, 
        INIT_ENVP
    ).expect("Failed to initialize init process from ELF");
    
    unsafe {
        *INIT_PROCESS.0.get() = MaybeUninit::new(pcb);
    }
}

pub fn get_initprocess() -> &'static Arc<PCB> {
    unsafe {
        (&*INIT_PROCESS.0.get()).assume_init_ref()
    }
}
