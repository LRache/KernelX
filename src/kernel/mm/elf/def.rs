use core::fmt::Display;

pub type Elf64Half = u16;
pub type Elf64Word = u32;
pub type Elf64Xword = u64;
pub type Elf64Addr = u64;
pub type Elf64Off = u64;

pub const EI_NIDENT: usize = 16;

pub const ELFMAG0: u8 = 0x7f;
pub const ELFMAG1: u8 = b'E';
pub const ELFMAG2: u8 = b'L';
pub const ELFMAG3: u8 = b'F';

pub const EI_MAG0: usize = 0;
pub const EI_MAG1: usize = 1;
pub const EI_MAG2: usize = 2;
pub const EI_MAG3: usize = 3;
pub const EI_CLASS: usize = 4;
pub const EI_DATA: usize = 5;

pub const ELFCLASSNONE: u8 = 0;
pub const ELFCLASS32: u8 = 1;
pub const ELFCLASS64: u8 = 2;

pub const ELFDATANONE: u8 = 0;
pub const ELFDATA2LSB: u8 = 1;
pub const ELFDATA2MSB: u8 = 2;

pub const ET_NONE: u16 = 0;
pub const ET_REL: u16 = 1;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const ET_CORE: u16 = 4;

// e_machine constants
pub const EM_RISCV: u16 = 243; // RISC-V

// ELF64文件头
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Ehdr {
    pub e_ident: [u8; EI_NIDENT],    // Magic number and other info
    pub e_type: Elf64Half,           // Object file type
    pub e_machine: Elf64Half,        // Architecture
    pub e_version: Elf64Word,        // Object file version
    pub e_entry: Elf64Addr,          // Entry point virtual address
    pub e_phoff: Elf64Off,           // Program header table file offset
    pub e_shoff: Elf64Off,           // Section header table file offset
    pub e_flags: Elf64Word,          // Processor-specific flags
    pub e_ehsize: Elf64Half,         // ELF header size in bytes
    pub e_phentsize: Elf64Half,      // Program header table entry size
    pub e_phnum: Elf64Half,          // Program header table entry count
    pub e_shentsize: Elf64Half,      // Section header table entry size
    pub e_shnum: Elf64Half,          // Section header table entry count
    pub e_shstrndx: Elf64Half,       // Section header string table index
}

pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_PHDR: u32 = 6;

pub const PF_X: u32 = 1 << 0;
pub const PF_W: u32 = 1 << 1;
pub const PF_R: u32 = 1 << 2;

// ELF64程序头
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Phdr {
    pub p_type  : Elf64Word , // Segment type
    pub p_flags : Elf64Word , // Segment flags
    pub p_offset: Elf64Off  , // Segment file offset
    pub p_vaddr : Elf64Addr , // Segment virtual address
    pub p_paddr : Elf64Addr , // Segment physical address
    pub p_filesz: Elf64Xword, // Segment size in file
    pub p_memsz : Elf64Xword, // Segment size in memory
    pub p_align : Elf64Xword, // Segment alignment
}

// // 段头类型 (sh_type)
// pub const SHT_NULL: u32 = 0;
// pub const SHT_PROGBITS: u32 = 1;
// pub const SHT_SYMTAB: u32 = 2;
// pub const SHT_STRTAB: u32 = 3;
// pub const SHT_RELA: u32 = 4;
// pub const SHT_HASH: u32 = 5;
// pub const SHT_DYNAMIC: u32 = 6;
// pub const SHT_NOTE: u32 = 7;
// pub const SHT_NOBITS: u32 = 8;
// pub const SHT_REL: u32 = 9;
// pub const SHT_SHLIB: u32 = 10;
// pub const SHT_DYNSYM: u32 = 11;

// // 段头标志 (sh_flags)
// pub const SHF_WRITE: u64 = 1 << 0;      // 可写
// pub const SHF_ALLOC: u64 = 1 << 1;      // 占用内存
// pub const SHF_EXECINSTR: u64 = 1 << 2;  // 可执行

// ELF64段头
// #[repr(C)]
// #[derive(Debug, Clone, Copy)]
// pub struct Elf64Shdr {
//     pub sh_name: Elf64Word,       // Section name (string tbl index)
//     pub sh_type: Elf64Word,       // Section type
//     pub sh_flags: Elf64Xword,     // Section flags
//     pub sh_addr: Elf64Addr,       // Section virtual addr at execution
//     pub sh_offset: Elf64Off,      // Section file offset
//     pub sh_size: Elf64Xword,      // Section size in bytes
//     pub sh_link: Elf64Word,       // Link to another section
//     pub sh_info: Elf64Word,       // Additional section information
//     pub sh_addralign: Elf64Xword, // Section alignment
//     pub sh_entsize: Elf64Xword,   // Entry size if section holds table
// }

impl Elf64Ehdr {
    pub fn is_valid_elf(&self) -> bool {
        self.e_ident[EI_MAG0] == ELFMAG0 &&
        self.e_ident[EI_MAG1] == ELFMAG1 &&
        self.e_ident[EI_MAG2] == ELFMAG2 &&
        self.e_ident[EI_MAG3] == ELFMAG3
    }
    
    pub fn is_64bit(&self) -> bool {
        self.e_ident[EI_CLASS] == ELFCLASS64
    }
    
    pub fn is_little_endian(&self) -> bool {
        self.e_ident[EI_DATA] == ELFDATA2LSB
    }
    
    pub fn is_riscv(&self) -> bool {
        self.e_machine == EM_RISCV
    }
    
    pub fn is_executable(&self) -> bool {
        self.e_type == ET_EXEC
    }

    pub fn is_dynamic(&self) -> bool {
        self.e_type == ET_DYN
    }
}

impl Elf64Phdr {
    pub const fn is_load(&self) -> bool {
        self.p_type == PT_LOAD
    }

    pub const fn is_dynamic(&self) -> bool {
        self.p_type == PT_DYNAMIC
    }

    pub const fn is_interp(&self) -> bool {
        self.p_type == PT_INTERP
    }

    pub const fn is_phdr(&self) -> bool {
        self.p_type == PT_PHDR
    }
    
    pub const fn is_readable(&self) -> bool {
        (self.p_flags & PF_R) != 0
    }
    
    pub const fn is_writable(&self) -> bool {
        (self.p_flags & PF_W) != 0
    }
    
    pub const fn is_executable(&self) -> bool {
        (self.p_flags & PF_X) != 0
    }
}

impl Display for Elf64Phdr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Elf64Phdr {{ p_type: {}, p_flags: {}, p_offset: {}, p_vaddr: {}, p_paddr: {}, p_filesz: {}, p_memsz: {}, p_align: {} }}",
               self.p_type, self.p_flags, self.p_offset, self.p_vaddr, self.p_paddr, self.p_filesz, self.p_memsz, self.p_align)
    }
}
