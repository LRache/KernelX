use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use elf::abi;
use elf::endian::LittleEndian;
use elf::file::{self, Class, FileHeader, parse_ident};
use elf::parse::{ParseAt, ParseError};
use elf::segment::ProgramHeader;

use crate::fs::file::{FileFlags, FileOps, RandomAccessFile};
use crate::fs::{Dentry, Perm, vfs};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{FanotifyEventMask, notify_fanotify, wait_fanotify_open_exec_permission};
use crate::kernel::mm::{AddrSpace, MapPerm, maparea};
use crate::{arch, println};

type ElfHeader = FileHeader<LittleEndian>;

#[derive(Debug, Clone, Copy)]
pub struct DynInfo {
    pub user_entry: usize,
    pub interpreter_base: usize,
    pub phdr_addr: usize,
    pub phent: u16,
    pub phnum: u16,
}

fn read_exact_at(file: &Arc<RandomAccessFile>, buf: &mut [u8], offset: usize) -> SysResult<()> {
    let mut read_len = 0;
    while read_len < buf.len() {
        let current_offset = offset.checked_add(read_len).ok_or(Errno::ENOEXEC)?;
        let n = file.pread(&mut buf[read_len..], current_offset)?;
        if n == 0 {
            return Err(Errno::ENOEXEC);
        }
        read_len += n;
    }
    Ok(())
}

fn read_ehdr(file: &Arc<RandomAccessFile>) -> SysResult<ElfHeader> {
    let mut ident_buf = [0u8; abi::EI_NIDENT];
    read_exact_at(file, &mut ident_buf, 0)?;

    let ident = errno_to_kernel(parse_ident::<LittleEndian>(&ident_buf))?;
    let tail_size = match ident.1 {
        Class::ELF32 => file::ELF32_EHDR_TAILSIZE,
        Class::ELF64 => file::ELF64_EHDR_TAILSIZE,
    };
    let mut tail_buf = [0u8; file::ELF64_EHDR_TAILSIZE];
    read_exact_at(file, &mut tail_buf[..tail_size], abi::EI_NIDENT)?;
    errno_to_kernel(FileHeader::parse_tail(ident, &tail_buf[..tail_size]))
}

fn read_phdr_table(file: &Arc<RandomAccessFile>, ehdr: &ElfHeader) -> Result<Vec<ProgramHeader>, Errno> {
    let ph_num = ehdr.e_phnum as usize;
    if ph_num == 0 {
        return Ok(Vec::new());
    }

    let phdr_size = errno_to_kernel(ProgramHeader::validate_entsize(ehdr.class, ehdr.e_phentsize as usize))?;
    let table_size = ph_num.checked_mul(phdr_size).ok_or(Errno::ENOEXEC)?;
    let mut table_buf = vec![0u8; table_size];

    read_exact_at(file, &mut table_buf, to_usize(ehdr.e_phoff)?)?;

    let mut offset = 0;
    let mut phdrs = Vec::with_capacity(ph_num);
    for _ in 0..ph_num {
        phdrs.push(errno_to_kernel(ProgramHeader::parse_at(
            ehdr.endianness,
            ehdr.class,
            &mut offset,
            &table_buf,
        ))?);
    }
    Ok(phdrs)
}

fn is_native(ehdr: &ElfHeader) -> bool {
    #[cfg(arch_riscv64)]
    {
        ehdr.e_machine == abi::EM_RISCV
    }
    #[cfg(arch_loongarch64)]
    {
        ehdr.e_machine == abi::EM_LOONGARCH
    }
}

fn is_executable(ehdr: &ElfHeader) -> bool {
    ehdr.e_type == abi::ET_EXEC
}

fn is_dynamic(ehdr: &ElfHeader) -> bool {
    ehdr.e_type == abi::ET_DYN
}

fn is_load(phdr: &ProgramHeader) -> bool {
    phdr.p_type == abi::PT_LOAD
}

pub fn load_elf(
    root: &Arc<Dentry>,
    file: &Arc<RandomAccessFile>,
    addrspace: &AddrSpace,
    perm: &Perm,
) -> Result<(usize, Option<DynInfo>), Errno> {
    let ehdr = read_ehdr(file)?;

    if ehdr.class != Class::ELF64 {
        println!("Unsupported ELF format: not 64-bit");
        return Err(Errno::ENOEXEC);
    }

    if !is_native(&ehdr) {
        println!(
            "Unsupported ELF format: e_machine={:#x} does not match kernel arch",
            ehdr.e_machine
        );
        return Err(Errno::ENOEXEC);
    }

    if !(is_dynamic(&ehdr) || is_executable(&ehdr)) {
        println!("Unsupported ELF type: e_type={:#x}", ehdr.e_type);
        return Err(Errno::ENOEXEC);
    }

    let addr_base = if is_executable(&ehdr) {
        0
    } else if is_dynamic(&ehdr) {
        config::USER_EXEC_ADDR_BASE
    } else {
        return Err(Errno::ENOEXEC);
    };

    let mut interpreter_path: Option<String> = None;
    let mut phdr_addr: Option<usize> = None;
    let phdrs = read_phdr_table(file, &ehdr)?;

    for phdr in &phdrs {
        if is_load(phdr) {
            load_program_from_file(phdr, file, addrspace, addr_base)?;
        } else if phdr.p_type == abi::PT_PHDR {
            phdr_addr = Some(to_usize(phdr.p_vaddr)?.checked_add(addr_base).ok_or(Errno::ENOEXEC)?);
        } else if phdr.p_type == abi::PT_INTERP {
            let mut buffer = vec![0u8; to_usize(phdr.p_filesz)?];
            read_exact_at(file, &mut buffer, to_usize(phdr.p_offset)?)?;

            if let Some(null_pos) = buffer.iter().position(|&x| x == 0) {
                buffer.truncate(null_pos);
            }

            if let Ok(path) = String::from_utf8(buffer) {
                interpreter_path = Some(path);
            } else {
                return Err(Errno::ENOEXEC);
            }
        }
    }

    let phdr_addr = phdr_addr.unwrap_or(0);

    if let Some(interpreter_path) = &interpreter_path {
        let (interpreter_base, interpreter_entry) = load_interpreter(root, &interpreter_path, addrspace, perm)?;

        let dyn_info = DynInfo {
            user_entry: to_usize(ehdr.e_entry)?.checked_add(addr_base).ok_or(Errno::ENOEXEC)?,
            interpreter_base,
            phdr_addr,
            phent: ehdr.e_phentsize,
            phnum: ehdr.e_phnum,
        };

        Ok((interpreter_entry, Some(dyn_info)))
    } else {
        Ok((
            to_usize(ehdr.e_entry)?.checked_add(addr_base).ok_or(Errno::ENOEXEC)?,
            None,
        ))
    }
}

pub fn load_program_from_file(
    phdr: &ProgramHeader,
    file: &Arc<RandomAccessFile>,
    addrspace: &AddrSpace,
    addr_base: usize,
) -> Result<(), Errno> {
    let mut perm = MapPerm::U | MapPerm::R;
    if phdr.p_flags & abi::PF_R != 0 {
        perm |= MapPerm::R;
    }
    if phdr.p_flags & abi::PF_W != 0 {
        perm |= MapPerm::W;
    }
    if phdr.p_flags & abi::PF_X != 0 {
        perm |= MapPerm::X;
    }

    let p_vaddr = to_usize(phdr.p_vaddr)?;
    let pgoff = p_vaddr % arch::PGSIZE;
    let ubase = p_vaddr.checked_add(addr_base).ok_or(Errno::ENOEXEC)? & !arch::PGMASK;
    let memory_size = to_usize(phdr.p_memsz)?.checked_add(pgoff).ok_or(Errno::ENOEXEC)?;
    let file_size = to_usize(phdr.p_filesz)?.checked_add(pgoff).ok_or(Errno::ENOEXEC)?;
    let file_offset = to_usize(phdr.p_offset)? & !arch::PGMASK;

    let area = maparea::ELFArea::new(ubase, perm, file.clone(), file_offset, file_size, memory_size);
    addrspace.map_area(ubase, Box::new(area))?;

    Ok(())
}

fn load_interpreter(root: &Arc<Dentry>, path: &str, addrspace: &AddrSpace, perm: &Perm) -> SysResult<(usize, usize)> {
    let file_flags = FileFlags::readonly();
    let file = vfs::openat_file(root, root, path, file_flags, perm)?;
    let file = file.downcast_arc::<RandomAccessFile>().map_err(|_| Errno::ENOEXEC)?;
    let fanotify_file: Arc<dyn FileOps> = file.clone();
    wait_fanotify_open_exec_permission(&fanotify_file)?;
    notify_fanotify(&fanotify_file, FanotifyEventMask::FAN_OPEN);
    notify_fanotify(&fanotify_file, FanotifyEventMask::FAN_OPEN_EXEC);

    let ehdr = read_ehdr(&file)?;

    if ehdr.class != Class::ELF64 || !is_native(&ehdr) {
        return Err(Errno::ENOEXEC);
    }

    if !is_dynamic(&ehdr) {
        return Err(Errno::ENOEXEC);
    }

    let addr_base = config::USER_LINKER_ADDR_BASE;
    let phdrs = read_phdr_table(&file, &ehdr)?;

    for phdr in &phdrs {
        if is_load(phdr) {
            load_program_from_file(phdr, &file, addrspace, addr_base)?;
        }
    }

    Ok((
        addr_base,
        to_usize(ehdr.e_entry)?.checked_add(addr_base).ok_or(Errno::ENOEXEC)?,
    ))
}

fn errno_to_kernel<T>(result: Result<T, ParseError>) -> SysResult<T> {
    result.map_err(|_| Errno::ENOEXEC)
}

fn to_usize(value: u64) -> SysResult<usize> {
    usize::try_from(value).map_err(|_| Errno::ENOEXEC)
}
