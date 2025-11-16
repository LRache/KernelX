use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::string::String;

use crate::fs::file::{File, FileOps, FileFlags, SeekWhence};
use crate::fs::vfs;
use crate::kernel::errno::Errno;
use crate::kernel::mm::elf::loaddyn::DynInfo;
use crate::kernel::mm::{maparea, AddrSpace, MapPerm};
use crate::kernel::config;
use crate::{arch, ktrace};
use crate::println;

use super::def::*;

pub fn read_ehdr(file: &Arc<File>) -> Result<Elf64Ehdr, Errno> {
    let mut header = [0u8; core::mem::size_of::<Elf64Ehdr>()];
    file.read(&mut header)?;
    
    let ehdr = unsafe {
        &*(header.as_ptr() as *const Elf64Ehdr)
    };
    
    Ok(*ehdr)
}

pub fn read_phdr(file: &Arc<File>) -> Result<Elf64Phdr, Errno> {
    let mut ph_buf = [0u8; core::mem::size_of::<Elf64Phdr>()];
    file.read(&mut ph_buf)?;
    
    let phdr = unsafe {
        &*(ph_buf.as_ptr() as *const Elf64Phdr)
    };
    
    Ok(*phdr)
}

pub fn load_elf(file: &Arc<File>, addrspace: &mut AddrSpace) -> Result<(usize, Option<DynInfo>), Errno> {
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

    ktrace!("PHDR offset: {:#x}, number of entries: {}", ph_offset, ph_num);

    let mut interpreter_path: Option<String> = None;
    let mut phdr_addr: Option<usize> = None;

    for i in 0..ph_num {
        file.seek((ph_offset + i * core::mem::size_of::<Elf64Phdr>()) as isize, SeekWhence::BEG)?;
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
        // ktrace!("Interpreter path: {}", interpreter_path);
        let (interpreter_base, interpreter_entry) = load_interpreter(&interpreter_path, addrspace)?;
    
        let dyn_info = DynInfo {
            user_entry: ehdr.e_entry as usize + addr_base,
            interpreter_base,
            phdr_addr,
            phent: ehdr.e_phentsize as u16,
            phnum: ehdr.e_phnum as u16,
        };

        Ok((interpreter_entry, Some(dyn_info)))
    } else {
        // Ok((load_exec(&ehdr, file, addrspace)?, None))
        Ok((ehdr.e_entry as usize + addr_base, None))
    }
}

pub fn load_loadable_phdr(
    ph_offset: usize,
    ph_num: usize,
    file: &Arc<File>,
    addrspace: &mut AddrSpace,
    addr_base: usize
) -> Result<(), Errno> {
    for i in 0..ph_num {
        file.seek((ph_offset + i * core::mem::size_of::<Elf64Phdr>()) as isize, SeekWhence::BEG)?;
        let phdr = read_phdr(file)?;
        
        if phdr.is_load() {
            load_program_from_file(&phdr, file, addrspace, addr_base)?;
        }
    }

    Ok(())
}

pub fn load_program_from_file(
    phdr: &Elf64Phdr,
    file: &Arc<File>,
    addrspace: &mut AddrSpace,
    addr_base: usize
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

    ktrace!("Loading program from file, phdr.p_vaddr={:#x}, phdr.p_memsz={:#x}, phdr.p_type={:#x}, addr_base={:#x}, perm={:?}", phdr.p_vaddr, phdr.p_memsz, phdr.p_type, addr_base, perm);

    let pgoff = phdr.p_vaddr as usize % arch::PGSIZE;
    let ubase = (phdr.p_vaddr as usize + addr_base) & !arch::PGMASK;
    let memory_size = phdr.p_memsz as usize + pgoff; // Aligned base to page
    let file_size = phdr.p_filesz as usize + pgoff;
    let file_offset = phdr.p_offset as usize & !arch::PGMASK;

    let area = maparea::ELFArea::new(ubase, perm, file.clone(), file_offset, file_size, memory_size);
    addrspace.map_area(ubase, Box::new(area))?;

    Ok(())
}

fn load_interpreter(path: &str, addrspace: &mut AddrSpace) -> Result<(usize, usize), Errno> {
    let file_flags = FileFlags::readonly();
    let file = vfs::open_file(path, file_flags).map_err(|_| {
        Errno::ENOENT
    })?;
    let file = Arc::new(file);
    
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
        addr_base
    )?;

    Ok((addr_base, ehdr.e_entry as usize + addr_base))
}
