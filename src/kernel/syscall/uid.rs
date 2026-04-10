use crate::kernel::errno::SysResult;
use crate::kernel::scheduler::current;

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
    pcb.set_euid(euid as u32);
    Ok(0)
}

pub fn setegid(egid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    pcb.set_egid(egid as u32);
    Ok(0)
}

pub fn setgid(gid: usize) -> SysResult<usize> {
    let pcb = current::pcb();
    let gid = gid as u32;
    if pcb.euid() == 0 {
        pcb.set_gid(gid);
        pcb.set_egid(gid);
    } else {
        pcb.set_egid(gid);
    }
    Ok(0)
}
