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
    let euid = euid as Uid;
    if pcb.euid() == 0 || euid == pcb.uid() || euid == pcb.euid() || euid == pcb.suid() {
        pcb.set_euid(euid);
    } else {
        return Err(Errno::EPERM);
    }
    Ok(0)
}

pub fn setegid(egid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let egid = egid as Uid;
    if pcb.euid() == 0 || egid == pcb.gid() || egid == pcb.egid() || egid == pcb.sgid() {
        pcb.set_egid(egid);
    } else {
        return Err(Errno::EPERM);
    }
    Ok(0)
}

pub fn setuid(uid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let uid = uid as Uid;
    if pcb.euid() == 0 {
        pcb.set_uid(uid);
        pcb.set_euid(uid);
        pcb.set_suid(uid);
    } else if uid == pcb.uid() || uid == pcb.suid() {
        pcb.set_euid(uid);
    } else {
        return Err(Errno::EPERM);
    }
    Ok(0)
}

pub fn setreuid(ruid: usize, euid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let old_ruid = pcb.uid();
    let old_euid = pcb.euid();
    let old_suid = pcb.suid();
    let privileged = old_euid == 0;

    let ruid = ruid as Uid;
    let new_ruid = if ruid == Uid::MAX {
        old_ruid
    } else {
        if !privileged && ruid != old_ruid && ruid != old_euid {
            return Err(Errno::EPERM);
        }
        ruid
    };

    let euid = euid as Uid;
    let new_euid = if euid == Uid::MAX {
        old_euid
    } else {
        if !privileged && euid != old_ruid && euid != old_euid && euid != old_suid {
            return Err(Errno::EPERM);
        }
        euid
    };

    if ruid != Uid::MAX {
        pcb.set_uid(new_ruid);
    }
    if euid != Uid::MAX {
        pcb.set_euid(new_euid);
    }

    if ruid != Uid::MAX || (euid != Uid::MAX && new_euid != old_ruid) {
        pcb.set_suid(new_euid);
    }

    Ok(0)
}

pub fn setresuid(ruid: usize, euid: usize, suid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let old_ruid = pcb.uid();
    let old_euid = pcb.euid();
    let old_suid = pcb.suid();
    let privileged = old_euid == 0;

    let ruid = ruid as Uid;
    let euid = euid as Uid;
    let suid = suid as Uid;

    if ruid != Uid::MAX && !privileged && ruid != old_ruid && ruid != old_euid && ruid != old_suid {
        return Err(Errno::EPERM);
    }
    if euid != Uid::MAX && !privileged && euid != old_ruid && euid != old_euid && euid != old_suid {
        return Err(Errno::EPERM);
    }
    if suid != Uid::MAX && !privileged && suid != old_ruid && suid != old_euid && suid != old_suid {
        return Err(Errno::EPERM);
    }

    if ruid != Uid::MAX {
        pcb.set_uid(ruid);
    }
    if euid != Uid::MAX {
        pcb.set_euid(euid);
    }
    if suid != Uid::MAX {
        pcb.set_suid(suid);
    }

    Ok(0)
}

pub fn setgid(gid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let gid = gid as Uid;
    if pcb.euid() == 0 {
        pcb.set_gid(gid);
        pcb.set_egid(gid);
        pcb.set_sgid(gid);
    } else if gid == pcb.gid() || gid == pcb.sgid() {
        pcb.set_egid(gid);
    } else {
        return Err(Errno::EPERM);
    }
    Ok(0)
}

pub fn setregid(rgid: usize, egid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let old_rgid = pcb.gid();
    let old_egid = pcb.egid();
    let old_sgid = pcb.sgid();
    let privileged = pcb.euid() == 0;

    let rgid = rgid as Uid;
    let new_rgid = if rgid == Uid::MAX {
        old_rgid
    } else {
        if !privileged && rgid != old_rgid && rgid != old_egid {
            return Err(Errno::EPERM);
        }
        rgid
    };

    let egid = egid as Uid;
    let new_egid = if egid == Uid::MAX {
        old_egid
    } else {
        if !privileged && egid != old_rgid && egid != old_egid && egid != old_sgid {
            return Err(Errno::EPERM);
        }
        egid
    };

    if rgid != Uid::MAX {
        pcb.set_gid(new_rgid);
    }
    if egid != Uid::MAX {
        pcb.set_egid(new_egid);
    }

    if rgid != Uid::MAX || (egid != Uid::MAX && new_egid != old_rgid) {
        pcb.set_sgid(new_egid);
    }

    Ok(0)
}

pub fn setresgid(rgid: usize, egid: usize, sgid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let old_rgid = pcb.gid();
    let old_egid = pcb.egid();
    let old_sgid = pcb.sgid();
    let privileged = pcb.euid() == 0;

    let rgid = rgid as Uid;
    let egid = egid as Uid;
    let sgid = sgid as Uid;

    if rgid != Uid::MAX && !privileged && rgid != old_rgid && rgid != old_egid && rgid != old_sgid {
        return Err(Errno::EPERM);
    }
    if egid != Uid::MAX && !privileged && egid != old_rgid && egid != old_egid && egid != old_sgid {
        return Err(Errno::EPERM);
    }
    if sgid != Uid::MAX && !privileged && sgid != old_rgid && sgid != old_egid && sgid != old_sgid {
        return Err(Errno::EPERM);
    }

    if rgid != Uid::MAX {
        pcb.set_gid(rgid);
    }
    if egid != Uid::MAX {
        pcb.set_egid(egid);
    }
    if sgid != Uid::MAX {
        pcb.set_sgid(sgid);
    }

    Ok(0)
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
