use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::fs::file::{File, FileOps, SeekWhence};
use crate::kernel::errno::Errno;
use crate::kernel::mm::elf::loaddyn::DynInfo;
use crate::kernel::mm::{maparea, AddrSpace, MapPerm};
use crate::{arch, ktrace};
use crate::println;
use super::def::*;
use super::loaddyn::load_dyn;

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
    
    if ehdr.is_executable() {
        Ok((load_exec(&ehdr, file, addrspace)?, None))
    } else if ehdr.is_dynamic() {
        let (userentry, dyn_info) = load_dyn(&ehdr, file, addrspace)?;
        Ok((userentry, Some(dyn_info)))
    } else {
        Err(Errno::ENOEXEC)
    }
}

fn load_exec(ehdr: &Elf64Ehdr, file: &Arc<File>, addrspace: &mut AddrSpace) -> Result<usize, Errno> {
    load_loadable_phdr(
        ehdr.e_phoff as usize, 
        ehdr.e_phnum as usize, 
        file, addrspace, 0
    )?;
    
    Ok(ehdr.e_entry as usize)
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
    let mut perm = MapPerm::U;
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
