use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;

use crate::fs::file::{File, FileFlags, SeekWhence};
use crate::fs::vfs;
use crate::kernel::errno::Errno;
use crate::kernel::mm::AddrSpace;
use crate::kernel::config;
use crate::ktrace;
use super::def::*;
use super::loader::*;

#[derive(Debug, Clone, Copy)]
pub struct DynInfo {
    pub user_entry: usize,
    pub interpreter_base: usize,
    pub phdr_addr: usize,
    pub phent: u16,
    pub phnum: u16,
}

pub fn load_dyn(ehdr: &Elf64Ehdr, file: &Arc<File>, addrspace: &mut AddrSpace) -> Result<(usize, DynInfo), Errno> {    
    let ph_offset = ehdr.e_phoff as usize;
    let ph_num = ehdr.e_phnum as usize;

    ktrace!("PHDR offset: {:#x}, number of entries: {}", ph_offset, ph_num);

    let addr_base = config::USER_EXEC_ADDR_BASE;
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
    
    let phdr_addr = phdr_addr.ok_or(Errno::ENOEXEC)?;
    
    if interpreter_path.is_none() {
        return Err(Errno::ENOEXEC);
    }

    let interrpreter_path = interpreter_path.unwrap();
    let (interpreter_base, interpreter_entry) = load_interpreter(&interrpreter_path, addrspace)?;
    
    let dyn_info = DynInfo {
        user_entry: ehdr.e_entry as usize + addr_base,
        interpreter_base,
        phdr_addr,
        phent: ehdr.e_phentsize as u16,
        phnum: ehdr.e_phnum as u16,
    };
    
    Ok((interpreter_entry, dyn_info))
}

fn load_interpreter(path: &str, addrspace: &mut AddrSpace) -> Result<(usize, usize), Errno> {
    let file_flags = FileFlags {
        writable: false,
        cloexec: false,
    };
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
