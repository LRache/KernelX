use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;
use crate::kernel::syscall::uptr::UArray;
use crate::kernel::uapi::Uid;

const NGROUPS_MAX: usize = 65536;

pub fn getuid() -> SysResult<usize> {
    Ok(current::pcb().uid() as usize)
}

pub fn geteuid() -> SysResult<usize> {
    Ok(current::pcb().euid() as usize)
}

pub fn getgid() -> SysResult<usize> {
    Ok(current::pcb().gid() as usize)
}

pub fn getegid() -> SysResult<usize> {
    Ok(current::pcb().egid() as usize)
}

pub fn seteuid(euid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    pcb.set_euid(euid as Uid);
    Ok(0)
}

pub fn setegid(egid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    pcb.set_egid(egid as Uid);
    Ok(0)
}

pub fn setuid(uid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let uid = uid as Uid;
    if pcb.euid() == 0 {
        pcb.set_uid(uid);
        pcb.set_euid(uid);
    } else {
        pcb.set_euid(uid);
    }
    Ok(0)
}

pub fn setreuid(ruid: usize, euid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let ruid = ruid as Uid;
    let euid = euid as Uid;
    if ruid != Uid::MAX {
        pcb.set_uid(ruid);
    }
    if euid != Uid::MAX {
        pcb.set_euid(euid);
    }
    Ok(0)
}

pub fn setresuid(ruid: usize, euid: usize, _suid: usize) -> SysResult<usize> {
    setreuid(ruid, euid)
}

pub fn setgid(gid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let gid = gid as Uid;
    if pcb.euid() == 0 {
        pcb.set_gid(gid);
        pcb.set_egid(gid);
    } else {
        pcb.set_egid(gid);
    }
    Ok(0)
}

pub fn setregid(rgid: usize, egid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let rgid = rgid as Uid;
    let egid = egid as Uid;
    if rgid != Uid::MAX {
        pcb.set_gid(rgid);
    }
    if egid != Uid::MAX {
        pcb.set_egid(egid);
    }
    Ok(0)
}

pub fn setresgid(rgid: usize, egid: usize, _sgid: usize) -> SysResult<usize> {
    setregid(rgid, egid)
}

pub fn getgroups(size: usize, groups: UArray<u32>) -> SysResult<usize> {
    let pcb = current::pcb();
    let supplementary_gids = pcb.supplementary_gids();
    let ngroups = supplementary_gids.len();

    if size == 0 {
        return Ok(ngroups);
    }
    if size < ngroups {
        return Err(Errno::EINVAL);
    }
    groups.write(0, &supplementary_gids)?;
    Ok(ngroups)
}

pub fn setgroups(size: usize, groups: UArray<u32>) -> SysResult<usize> {
    if current::pcb().euid() != 0 {
        return Err(Errno::EPERM);
    }
    if size > NGROUPS_MAX || size > i32::MAX as usize {
        return Err(Errno::EINVAL);
    }

    let mut supplementary_gids = Vec::with_capacity(size);
    supplementary_gids.resize(size, 0);
    if size > 0 {
        groups.read(0, &mut supplementary_gids)?;
    }

    current::pcb().set_supplementary_gids(supplementary_gids);
    Ok(0)
}
