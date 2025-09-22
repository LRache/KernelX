use crate::kernel::errno::SysResult;

pub fn getuid() -> SysResult<usize> {
    Ok(0)
}

pub fn geteuid() -> SysResult<usize> {
    Ok(0)
}

pub fn getgid() -> SysResult<usize> {
    Ok(0)
}

pub fn getegid() -> SysResult<usize> {
    Ok(0)
}
