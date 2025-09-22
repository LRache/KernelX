use crate::kernel::errno::Errno;
use crate::copy_to_user;

pub fn set_robust_list() -> Result<usize, Errno> {
    // This syscall is a no-op in the current implementation.
    // It is provided for compatibility with the Linux API.
    Ok(0)
}

pub fn rseq() -> Result<usize, Errno> {
    // This syscall is a no-op in the current implementation.
    // It is provided for compatibility with the Linux API.
    Ok(0)
}

#[repr(C)]
pub struct Utsname {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

impl Utsname {
    pub fn new() -> Self {
        let mut ustname = Utsname {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
            domainname: [0; 65],
        };
        let sysname = b"KernelX";
        ustname.sysname[..sysname.len()].copy_from_slice(sysname);

        let release = option_env!("KERNELX_RELEASE").unwrap_or("0.1.0");
        ustname.release[..release.len()].copy_from_slice(release.as_bytes());

        let machine = b"riscv64";
        ustname.machine[..machine.len()].copy_from_slice(machine);

        ustname
    }
}

pub fn newuname(uptr_uname: usize) -> Result<usize, Errno> {
    let ustname = Utsname::new();

    copy_to_user!(uptr_uname, ustname)?;

    Ok(0)
}
