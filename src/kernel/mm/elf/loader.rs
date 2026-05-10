use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;

use crate::fs::file::{FileFlags, FileOps, RandomAccessFile, SeekWhence};
use crate::fs::{Dentry, Perm, vfs};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{FanotifyEventMask, notify_fanotify, wait_fanotify_open_exec_permission};
use crate::kernel::mm::{AddrSpace, MapPerm, maparea};
use crate::{arch, ktrace, println};

use super::def::*;

#[derive(Debug, Clone, Copy)]
pub struct DynInfo {
    pub user_entry: usize,
    pub interpreter_base: usize,
    pub phdr_addr: usize,
    pub phent: u16,
    pub phnum: u16,
}

pub fn read_ehdr(file: &Arc<RandomAccessFile>) -> Result<Elf64Ehdr, Errno> {
    let mut header = [0u8; core::mem::size_of::<Elf64Ehdr>()];
    file.read(&mut header)?;

    let ehdr = unsafe { &*(header.as_ptr() as *const Elf64Ehdr) };

    Ok(*ehdr)
}

pub fn read_phdr(file: &Arc<RandomAccessFile>) -> Result<Elf64Phdr, Errno> {
    let mut ph_buf = [0u8; core::mem::size_of::<Elf64Phdr>()];
    file.read(&mut ph_buf)?;

    let phdr = unsafe { &*(ph_buf.as_ptr() as *const Elf64Phdr) };

    Ok(*phdr)
}

pub fn load_elf(
    root: &Arc<Dentry>,
    file: &Arc<RandomAccessFile>,
    addrspace: &AddrSpace,
    perm: &Perm,
) -> Result<(usize, Option<DynInfo>), Errno> {
    let ehdr = read_ehdr(file)?;

    if !ehdr.is_valid_elf() {
        ktrace!("Invalid ELF header: {:?}", ehdr.e_ident);
        return Err(Errno::ENOEXEC);
    }

    if !ehdr.is_64bit() {
        println!("Unsupported ELF format: not 64-bit");
        return Err(Errno::ENOEXEC);
    }

    if !ehdr.is_little_endian() {
        return Err(Errno::ENOEXEC);
    }

    if !ehdr.is_riscv() {
        println!("Unsupported ELF format: not RISC-V");
        return Err(Errno::ENOEXEC);
    }

    if !(ehdr.is_dynamic() || ehdr.is_executable()) {
        println!("Unsupported ELF type: e_type={:#x}", ehdr.e_type);
        return Err(Errno::ENOEXEC);
    }

    let addr_base = if ehdr.is_executable() {
        0
    } else if ehdr.is_dynamic() {
        config::USER_EXEC_ADDR_BASE
    } else {
        return Err(Errno::ENOEXEC);
    };

    let ph_offset = ehdr.e_phoff as usize;
    let ph_num = ehdr.e_phnum as usize;

    let mut interpreter_path: Option<String> = None;
    let mut phdr_addr: Option<usize> = None;

    for i in 0..ph_num {
        file.seek(
            (ph_offset + i * core::mem::size_of::<Elf64Phdr>()) as isize,
            SeekWhence::BEG,
        )?;
        let phdr = read_phdr(file)?;

        if phdr.is_load() {
            load_program_from_file(&phdr, file, addrspace, addr_base)?;
        } else if phdr.is_phdr() {
            phdr_addr = Some(phdr.p_vaddr as usize + addr_base);
        } else if phdr.is_interp() {
            file.seek(phdr.p_offset as isize, SeekWhence::BEG)?;
            let mut buffer = vec![0u8; phdr.p_filesz as usize];
            file.read(&mut buffer)?;

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
            user_entry: ehdr.e_entry as usize + addr_base,
            interpreter_base,
            phdr_addr,
            phent: ehdr.e_phentsize as u16,
            phnum: ehdr.e_phnum as u16,
        };

        Ok((interpreter_entry, Some(dyn_info)))
    } else {
        Ok((ehdr.e_entry as usize + addr_base, None))
    }
}

pub fn load_loadable_phdr(
    ph_offset: usize,
    ph_num: usize,
    file: &Arc<RandomAccessFile>,
    addrspace: &AddrSpace,
    addr_base: usize,
) -> Result<(), Errno> {
    for i in 0..ph_num {
        file.seek(
            (ph_offset + i * core::mem::size_of::<Elf64Phdr>()) as isize,
            SeekWhence::BEG,
        )?;
        let phdr = read_phdr(file)?;

        if phdr.is_load() {
            load_program_from_file(&phdr, file, addrspace, addr_base)?;
        }
    }

    Ok(())
}

pub fn load_program_from_file(
    phdr: &Elf64Phdr,
    file: &Arc<RandomAccessFile>,
    addrspace: &AddrSpace,
    addr_base: usize,
) -> Result<(), Errno> {
    let mut perm = MapPerm::U | MapPerm::R;
    if phdr.is_readable() {
        perm |= MapPerm::R;
    }
    if phdr.is_writable() {
        perm |= MapPerm::W;
    }
    if phdr.is_executable() {
        perm |= MapPerm::X;
    }

    let pgoff = phdr.p_vaddr as usize % arch::PGSIZE;
    let ubase = (phdr.p_vaddr as usize + addr_base) & !arch::PGMASK;
    let memory_size = phdr.p_memsz as usize + pgoff; // Aligned base to page
    let file_size = phdr.p_filesz as usize + pgoff;
    let file_offset = phdr.p_offset as usize & !arch::PGMASK;

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

    if !ehdr.is_valid_elf() || !ehdr.is_64bit() || !ehdr.is_riscv() {
        return Err(Errno::ENOEXEC);
    }

    if !ehdr.is_dynamic() {
        return Err(Errno::ENOEXEC);
    }

    let addr_base = config::USER_LINKER_ADDR_BASE;

    load_loadable_phdr(
        ehdr.e_phoff as usize,
        ehdr.e_phnum as usize,
        &file,
        addrspace,
        addr_base,
    )?;

    Ok((addr_base, ehdr.e_entry as usize + addr_base))
}
