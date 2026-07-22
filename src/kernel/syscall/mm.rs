use alloc::boxed::Box;
use alloc::sync::Arc;
use bitflags::bitflags;

use crate::arch;

use crate::fs::file::FileMmapRequest;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::{Area, PrivateAnonymousArea, SharedAnonymousArea};
use crate::kernel::mm::{AddrSpace, MapPerm, MmapPlacement};
use crate::kernel::scheduler::*;
use crate::kernel::syscall::SyscallRet;
use crate::kernel::task;

use super::common::{BUFFER_SIZE, IOVec, IOVecCursor};
use super::uptr::{UPtr, UserPointer};

const PROCESS_VM_IOV_MAX: usize = 1024;

fn can_process_vm_target(target: &task::PCB) -> bool {
    let caller = current::pcb();
    if caller.euid() == 0 {
        return true;
    }

    let caller_uid = caller.uid();
    let caller_euid = caller.euid();
    caller_uid == target.uid()
        || caller_uid == target.suid()
        || caller_euid == target.uid()
        || caller_euid == target.suid()
}

fn process_vm_target_addrspace(pid: usize) -> SysResult<Arc<AddrSpace>> {
    let tid = Tid::try_from(pid).map_err(|_| Errno::ESRCH)?;
    if tid <= 0 {
        return Err(Errno::ESRCH);
    }

    let target = task::manager::get(tid).ok_or(Errno::ESRCH)?;
    if target.state().lock().is_dead() || target.parent().is_exited() {
        return Err(Errno::ESRCH);
    }
    if !can_process_vm_target(target.parent()) {
        return Err(Errno::EPERM);
    }

    Ok(Arc::clone(target.get_addrspace()))
}

fn transfer_process_vm_iovecs(
    src_addrspace: &AddrSpace,
    src_iov: UPtr<IOVec>,
    src_iovcnt: usize,
    dst_addrspace: &AddrSpace,
    dst_iov: UPtr<IOVec>,
    dst_iovcnt: usize,
) -> SyscallRet {
    let mut src_cursor = IOVecCursor::new(src_iov, src_iovcnt);
    let mut dst_cursor = IOVecCursor::new(dst_iov, dst_iovcnt);
    let mut copied = 0usize;
    let mut buffer = [0u8; BUFFER_SIZE];

    loop {
        let src_remaining = match src_cursor.remaining() {
            Ok(Some(remaining)) => remaining,
            Ok(None) => break,
            Err(errno) => {
                if copied == 0 {
                    return Err(errno);
                }
                return Ok(copied);
            }
        };
        let dst_remaining = match dst_cursor.remaining() {
            Ok(Some(remaining)) => remaining,
            Ok(None) => break,
            Err(errno) => {
                if copied == 0 {
                    return Err(errno);
                }
                return Ok(copied);
            }
        };

        let to_copy = core::cmp::min(core::cmp::min(src_remaining, dst_remaining), BUFFER_SIZE);
        let Some(src_addr) = src_cursor.current_addr() else {
            if copied == 0 {
                return Err(Errno::EFAULT);
            }
            return Ok(copied);
        };
        let Some(dst_addr) = dst_cursor.current_addr() else {
            if copied == 0 {
                return Err(Errno::EFAULT);
            }
            return Ok(copied);
        };

        if src_addrspace
            .copy_from_user_buffer(src_addr, &mut buffer[..to_copy])
            .is_err()
        {
            if copied == 0 {
                return Err(Errno::EFAULT);
            }
            return Ok(copied);
        }
        if dst_addrspace.copy_to_user_buffer(dst_addr, &buffer[..to_copy]).is_err() {
            if copied == 0 {
                return Err(Errno::EFAULT);
            }
            return Ok(copied);
        }

        src_cursor.advance(to_copy);
        dst_cursor.advance(to_copy);
        copied += to_copy;
    }

    Ok(copied)
}

fn validate_process_vm_iovcnt(iovcnt: usize) -> SysResult<()> {
    if (iovcnt as isize) < 0 || iovcnt > PROCESS_VM_IOV_MAX {
        Err(Errno::EINVAL)
    } else {
        Ok(())
    }
}

fn process_vm(
    pid: usize,
    local_iov: UPtr<IOVec>,
    liovcnt: usize,
    remote_iov: UPtr<IOVec>,
    riovcnt: usize,
    flags: usize,
    write_remote: bool,
) -> SyscallRet {
    if flags != 0 {
        return Err(Errno::EINVAL);
    }
    validate_process_vm_iovcnt(liovcnt)?;
    validate_process_vm_iovcnt(riovcnt)?;
    if liovcnt == 0 || riovcnt == 0 {
        return Ok(0);
    }

    if local_iov.is_null() || remote_iov.is_null() {
        return Err(Errno::EFAULT);
    }

    let local_addrspace = Arc::clone(current::addrspace());
    let remote_addrspace = process_vm_target_addrspace(pid)?;

    if write_remote {
        transfer_process_vm_iovecs(
            &local_addrspace,
            local_iov,
            liovcnt,
            &remote_addrspace,
            remote_iov,
            riovcnt,
        )
    } else {
        transfer_process_vm_iovecs(
            &remote_addrspace,
            remote_iov,
            riovcnt,
            &local_addrspace,
            local_iov,
            liovcnt,
        )
    }
}

pub fn process_vm_readv(
    pid: usize,
    local_iov: UPtr<IOVec>,
    liovcnt: usize,
    remote_iov: UPtr<IOVec>,
    riovcnt: usize,
    flags: usize,
) -> SyscallRet {
    process_vm(pid, local_iov, liovcnt, remote_iov, riovcnt, flags, false)
}

pub fn process_vm_writev(
    pid: usize,
    local_iov: UPtr<IOVec>,
    liovcnt: usize,
    remote_iov: UPtr<IOVec>,
    riovcnt: usize,
    flags: usize,
) -> SyscallRet {
    process_vm(pid, local_iov, liovcnt, remote_iov, riovcnt, flags, true)
}

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

    let area: Box<dyn Area> = if flags.contains(MMapFlags::ANONYMOUS) {
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

        file.mmap_area(FileMmapRequest {
            shared,
            perm,
            offset,
            length,
        })?
    };

    let addrspace = current::addrspace();
    let placement = if fixed_noreplace {
        MmapPlacement::FixedNoReplace(addr)
    } else if fixed {
        MmapPlacement::Fixed(addr)
    } else {
        MmapPlacement::Hint(addr)
    };
    let ubase = addrspace.mmap_area(placement, area)?;

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

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct MRemapFlags: usize {
        const MAYMOVE  = 1;
        const FIXED    = 2;
        const DONTUNMAP = 4;
    }
}

pub fn mremap(old_addr: usize, old_size: usize, new_size: usize, flags: usize, _new_addr: usize) -> SyscallRet {
    let flags = MRemapFlags::from_bits(flags & 0x7).ok_or(Errno::EINVAL)?;

    if old_addr % arch::PGSIZE != 0 || new_size == 0 {
        return Err(Errno::EINVAL);
    }

    let old_pages = arch::page_count(old_size);
    let new_pages = arch::page_count(new_size);

    if new_pages == old_pages {
        return Ok(old_addr);
    }

    if new_pages < old_pages {
        let tail_addr = old_addr + new_pages * arch::PGSIZE;
        let tail_pages = old_pages - new_pages;
        current::addrspace().unmap_area(tail_addr, tail_pages)?;
        return Ok(old_addr);
    }

    if !flags.contains(MRemapFlags::MAYMOVE) {
        return Err(Errno::ENOMEM);
    }

    let new_addr = mmap(
        0,
        new_pages * arch::PGSIZE,
        0x3,  /* R|W */
        0x22, /* PRIVATE|ANON */
        usize::MAX,
        0,
    )?;

    let copy_len = old_pages * arch::PGSIZE;
    let addrspace = current::addrspace();
    for offset in (0..copy_len).step_by(arch::PGSIZE) {
        let _ = addrspace.with_translated_read_write(old_addr + offset, new_addr + offset, arch::PGSIZE, |src, dst| {
            dst.copy_from_slice(src);
        });
    }

    addrspace.unmap_area(old_addr, old_pages)?;

    Ok(new_addr)
}

pub fn mlock(_start: usize, _length: usize) -> SyscallRet {
    Ok(0)
}
