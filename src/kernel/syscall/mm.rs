use alloc::boxed::Box;
use bitflags::bitflags;

use crate::arch;
use crate::fs::file::{FileOps, RandomAccessFile};
use crate::kernel::errno::Errno;
use crate::kernel::mm::maparea::{
    Area, PrivateAnonymousArea, PrivateFileMapArea, SharedAnonymousArea, SharedFileMapArea,
};
use crate::kernel::mm::MapPerm;
use crate::kernel::scheduler::*;
use crate::kernel::syscall::SyscallRet;

pub fn brk(brk: usize) -> SyscallRet {
    let r = current::addrspace().increase_userbrk(brk);
    r
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MMapProt: usize {
        const READ  = 0x1;
        const WRITE = 0x2;
        const EXEC  = 0x4;
    }
}

impl Into<MapPerm> for MMapProt {
    fn into(self) -> MapPerm {
        let mut perm = MapPerm::U;
        if self.contains(MMapProt::READ) {
            perm |= MapPerm::R;
        }
        if self.contains(MMapProt::WRITE) {
            perm |= MapPerm::W;
        }
        if self.contains(MMapProt::EXEC) {
            perm |= MapPerm::X;
        }
        perm
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MMapFlags: usize {
        const SHARED    = 0x001; // Shared mapping
        const PRIVATE   = 0x002; // Private mapping
        const FIXED     = 0x010; // Fixed address mapping
        const ANONYMOUS = 0x020; // Anonymous mapping
        const DENYWRITE = 0x800; // Deny write access
        const NORESERVE = 0x4000; // Do not reserve swap space
        const MAP_STACK = 0x20000;
        const FIXED_NOREPLACE = 0x100000; // Fixed address mapping without replacing existing mappings
    }
}

pub fn mmap(addr: usize, length: usize, prot: usize, flags: usize, fd: usize, offset: usize) -> SyscallRet {
    let flags = MMapFlags::from_bits(flags).ok_or(Errno::EOPNOTSUPP)?;
    let fixed = flags.contains(MMapFlags::FIXED);
    let fixed_noreplace = flags.contains(MMapFlags::FIXED_NOREPLACE);

    if addr % arch::PGSIZE != 0 || length == 0 {
        return Err(Errno::EINVAL);
    }

    if addr >= arch::USEREND {
        return Err(Errno::EINVAL);
    }

    let page_count = arch::page_count(length);
    let map_size = page_count.checked_mul(arch::PGSIZE).ok_or(Errno::ENOMEM)?;
    if addr.checked_add(map_size - 1).ok_or(Errno::EINVAL)? > arch::USEREND {
        return Err(Errno::EINVAL);
    }

    let prot = MMapProt::from_bits(prot).ok_or(Errno::EINVAL)?;

    let mut perm = MapPerm::U;
    if prot.contains(MMapProt::READ) {
        perm |= MapPerm::R;
    }
    if prot.contains(MMapProt::WRITE) {
        perm |= MapPerm::W;
    }
    if prot.contains(MMapProt::EXEC) {
        perm |= MapPerm::X;
    }

    let shared = if flags.contains(MMapFlags::SHARED) {
        true
    } else if flags.contains(MMapFlags::PRIVATE) {
        false
    } else {
        return Err(Errno::EINVAL);
    };

    let mut area: Box<dyn Area> = if flags.contains(MMapFlags::ANONYMOUS) {
        if fixed_noreplace
            && current::addrspace().with_map_manager_mut(|map_manager| map_manager.is_range_mapped(addr, map_size))
        {
            return Err(Errno::EEXIST);
        }

        if shared {
            Box::new(SharedAnonymousArea::new(0, perm, page_count))
        } else {
            Box::new(PrivateAnonymousArea::new(0, perm, page_count))
        }
    } else {
        let file = current::fdtable().lock().get(fd)?;

        if offset % arch::PGSIZE != 0 {
            return Err(Errno::EINVAL);
        }

        let file = file.downcast_arc::<RandomAccessFile>().map_err(|_| Errno::EBADF)?;

        if !file.flags.readable {
            return Err(Errno::EACCES);
        }
        if flags.contains(MMapFlags::SHARED) && prot.contains(MMapProt::WRITE) && !file.flags.writable {
            return Err(Errno::EACCES);
        }

        if fixed_noreplace
            && current::addrspace().with_map_manager_mut(|map_manager| map_manager.is_range_mapped(addr, map_size))
        {
            return Err(Errno::EEXIST);
        }

        let dentry = file.get_dentry().unwrap();
        let inode = file.get_inode().unwrap().clone();
        let index = dentry.get_inode_index();

        if shared {
            // if length % arch::PGSIZE != 0 {
            //     return Err(Errno::EINVAL);
            // }

            Box::new(SharedFileMapArea::new(
                0,
                perm,
                inode,
                index,
                offset,
                page_count,
                dentry.get_path(),
            ))
        } else {
            Box::new(PrivateFileMapArea::new(0, perm, file, offset, length))
        }
    };

    let addrspace = current::addrspace();
    let ubase = addrspace.with_map_manager_mut(|map_manager| {
        if fixed_noreplace {
            if map_manager.is_range_mapped(addr, map_size) {
                return Err(Errno::EEXIST);
            }
            Ok(addr)
        } else if fixed {
            Ok(addr)
        } else if addr == 0 || map_manager.is_range_mapped(addr, map_size) {
            map_manager.find_mmap_ubase(page_count).ok_or(Errno::ENOMEM)
        } else {
            Ok(addr)
        }
    })?;

    area.set_ubase(ubase);
    if fixed && !fixed_noreplace {
        addrspace.map_area_fixed(ubase, area)?;
    } else {
        addrspace.map_area(ubase, area)?;
    }

    Ok(ubase)
}

pub fn munmap(addr: usize, length: usize) -> SyscallRet {
    if addr % arch::PGSIZE != 0 || length == 0 {
        return Err(Errno::EINVAL);
    }

    let page_count = arch::page_count(length);

    current::addrspace().unmap_area(addr, page_count)?;

    Ok(0)
}

pub fn mprotect(addr: usize, length: usize, prot: usize) -> SyscallRet {
    let prot = MMapProt::from_bits(prot).ok_or(Errno::EINVAL)?;

    if length == 0 || length % arch::PGSIZE != 0 || addr % arch::PGSIZE != 0 {
        return Err(Errno::EINVAL);
    }

    // Align up length to page size
    let page_count = (length + arch::PGSIZE - 1) / arch::PGSIZE;

    current::addrspace().set_area_perm(addr, page_count, prot.into())?;

    Ok(0)
}

pub fn msync(_addr: usize, _length: usize, _flags: usize) -> SyscallRet {
    // Currently no-op
    Ok(0)
}

pub fn madvise() -> SyscallRet {
    // Currently no-op
    Ok(0)
}

pub fn mlock(_start: usize, _length: usize) -> SyscallRet {
    // Currently no-op
    Ok(0)
}
