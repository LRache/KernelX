use spin::Lazy;
use alloc::sync::Arc;

use crate::kernel::task::pcb::PCB;
use crate::fs::vfs;
use crate::fs::file::FileFlags;

const INITPATH: &'static str = match option_env!("KERNELX_INITPATH") {
    Some(path) => path,
    None => "/glibc/basic/brk",
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

static INIT_PROCESS: Lazy<Arc<PCB>> = Lazy::new(|| {
    let initfile = vfs::open(INITPATH, FileFlags::dontcare()).expect("Failed to open init file");
    let pcb = PCB::new_initprocess(initfile, INIT_ARGV, INIT_ENVP).expect("Failed to initialize init process from ELF");
    pcb
});

pub fn get_init_process() -> &'static Arc<PCB> {
    &INIT_PROCESS
}
