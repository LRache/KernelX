use alloc::vec::Vec;
use alloc::vec;

use crate::kernel::config;
use crate::kernel::mm::elf::def::{Elf64Ehdr, Elf64Phdr};
use crate::arch::{self, PageTableTrait};
use crate::kernel::mm::MapPerm;
use crate::klib::initcell::InitedCell;

use super::PhysPageFrame;

unsafe extern "C" {
    static __vdso_start: u8;
    static __vdso_end: u8;
}

mod addr {
    // include!("/home/rache/code/KernelX/./vdso/build/riscv64/symbols.inc");
    // include!(concat!(env!("OUT_DIR"), "/symbols.inc"));
    include!(concat!(env!("KERNELX_HOME"), "/vdso/build/", env!("ARCH"), env!("ARCH_BITS"), "/symbols.inc"));
}

fn vdso_start() -> usize {
    core::ptr::addr_of!(__vdso_start) as usize
}

#[inline(always)]
pub fn addr_of(symbol: &str) -> usize {
    match symbol {
        "sigreturn_trampoline" => addr::__vdso_sigreturn_trampoline,
        _ => panic!("Unknown VDSO symbol: {}", symbol),
    }
}

struct LoadedProgram {
    ubase: usize,
    pages: Vec<PhysPageFrame>,
}

struct VDSOInfo {
    programs: Vec<LoadedProgram>,
}

static VDSO: InitedCell<VDSOInfo> = InitedCell::new();

fn load_programs(ehdr: &Elf64Ehdr) -> Vec<LoadedProgram> {
    let ph_addr = vdso_start() + ehdr.e_phoff as usize;
    let mut loaded_programs = Vec::new();
    
    for i in 0..ehdr.e_phnum {
        let phdr = unsafe { (ph_addr as *const Elf64Phdr).add(i as usize).as_ref().unwrap() };
        if !phdr.is_load() {
            continue;
        }
        
        let mut pages = vec![];
        let mut loaded = 0;
        let mut copied = 0;
        let memsz = phdr.p_memsz as usize;
        let filesz = phdr.p_filesz as usize; 
        let program_start = (vdso_start() + phdr.p_offset as usize) as *const u8;

        // Load unaligned
        let pageoff = phdr.p_vaddr as usize & arch::PGMASK;
        if pageoff != 0 {
            let page = PhysPageFrame::alloc_zeroed();
            let to_copy = core::cmp::min(arch::PGSIZE - pageoff, filesz);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    program_start, 
                    page.ptr().add(pageoff), 
                    to_copy,
                );
            }

            loaded += to_copy;
            copied += to_copy;
            pages.push(page);
        }
        
        while loaded < filesz {
            let page = PhysPageFrame::alloc_zeroed();
            let to_copy = core::cmp::min(arch::PGSIZE, filesz - copied);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    program_start.add(loaded), 
                    page.ptr(), 
                    to_copy,
                );
            }
            
            pages.push(page);
            copied += to_copy;
            loaded += arch::PGSIZE;
        }

        while loaded < memsz {
            let page = PhysPageFrame::alloc_zeroed();
            pages.push(page);
            loaded += arch::PGSIZE;
        }

        loaded_programs.push(LoadedProgram {
            ubase: phdr.p_vaddr as usize,
            pages,
        });
    }

    loaded_programs
}

pub fn init() {
    let ehdr = unsafe {(vdso_start() as *const Elf64Ehdr).as_ref().unwrap()};
    
    if !ehdr.is_valid_elf() || !ehdr.is_64bit() || !ehdr.is_little_endian() || !ehdr.is_riscv() || !ehdr.is_dynamic() {
        panic!("Invalid VDSO ELF header: {:?}", ehdr);
    }

    // load VDSO's program to memory
    let loaded_programs = load_programs(ehdr);

    VDSO.init(VDSOInfo { programs: loaded_programs });
}

pub fn map_to_pagetale(pagetable: &mut arch::PageTable) {
    for program in &VDSO.programs {
        let mut uaddr = program.ubase + config::VDSO_BASE;
        for page in program.pages.iter() {
            pagetable.mmap(uaddr, page.get_page(), MapPerm::R | MapPerm::X | MapPerm::U);
            uaddr += arch::PGSIZE;
        }
    }
}
