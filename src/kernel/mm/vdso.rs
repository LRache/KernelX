use alloc::vec;
use alloc::vec::Vec;

use elf::endian::LittleEndian;
use elf::file::Class;
use elf::{ElfBytes, abi};

use crate::arch::{self, PageTableTrait};
use crate::kernel::config;
use crate::kernel::mm::MapPerm;
use crate::klib::initcell::InitedCell;

use super::PhysPageFrame;

type VdsoElf<'a> = ElfBytes<'a, LittleEndian>;

unsafe extern "C" {
    static __vdso_start: u8;
    static __vdso_end: u8;
}

mod addr {
    include!(concat!(
        env!("KERNELX_HOME"),
        "/vdso/build/",
        env!("ARCH"),
        env!("ARCH_BITS"),
        "/symbols.inc"
    ));
}

#[unsafe(link_section = ".data.init")]
static VDSO_BYTES: &[u8] = include_bytes!(concat!(
    env!("KERNELX_HOME"),
    "/vdso/build/",
    env!("ARCH"),
    env!("ARCH_BITS"),
    "/lib/libvdso.so"
));

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

static VDSO: InitedCell<VDSOInfo> = InitedCell::uninit();

fn load_programs(file: &VdsoElf<'_>) -> Vec<LoadedProgram> {
    let phdrs = file.segments().expect("Invalid VDSO ELF program headers");
    let mut loaded_programs = Vec::new();

    for phdr in phdrs.iter() {
        if phdr.p_type != abi::PT_LOAD {
            continue;
        }

        let mut pages = vec![];
        let mut loaded = 0;
        let mut copied = 0;
        let memsz = to_usize(phdr.p_memsz);
        let filesz = to_usize(phdr.p_filesz);
        let offset = to_usize(phdr.p_offset);
        let end = offset.checked_add(filesz).expect("Invalid VDSO ELF segment range");
        let program = VDSO_BYTES.get(offset..end).expect("Invalid VDSO ELF segment range");

        // Load unaligned
        let pageoff = to_usize(phdr.p_vaddr) & arch::PGMASK;
        if pageoff != 0 {
            let page = PhysPageFrame::alloc_zeroed();
            let to_copy = core::cmp::min(arch::PGSIZE - pageoff, filesz);
            page.slice()[pageoff..pageoff + to_copy].copy_from_slice(&program[..to_copy]);

            loaded += to_copy;
            copied += to_copy;
            pages.push(page);
        }

        while loaded < filesz {
            let page = PhysPageFrame::alloc_zeroed();
            let to_copy = core::cmp::min(arch::PGSIZE, filesz - copied);
            page.slice()[..to_copy].copy_from_slice(&program[copied..copied + to_copy]);

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
            ubase: to_usize(phdr.p_vaddr),
            pages,
        });
    }

    loaded_programs
}

#[unsafe(link_section = ".text.init")]
pub fn init() {
    let file = ElfBytes::<LittleEndian>::minimal_parse(VDSO_BYTES).expect("Invalid VDSO ELF");

    if file.ehdr.class != Class::ELF64 || !is_native(file.ehdr.e_machine) || file.ehdr.e_type != abi::ET_DYN {
        panic!("Invalid VDSO ELF header: {:?}", file.ehdr);
    }

    // load VDSO's program to memory
    let loaded_programs = load_programs(&file);

    VDSO.init(VDSOInfo {
        programs: loaded_programs,
    });
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

fn to_usize(value: u64) -> usize {
    usize::try_from(value).expect("VDSO ELF field does not fit in usize")
}

fn is_native(e_machine: u16) -> bool {
    #[cfg(arch_riscv64)]
    {
        e_machine == abi::EM_RISCV
    }
    #[cfg(arch_loongarch64)]
    {
        e_machine == abi::EM_LOONGARCH
    }
}
