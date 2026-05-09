use alloc::vec::Vec;
use num_enum::TryFromPrimitive;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;
use crate::kernel::syscall::uptr::{UArray, UPtr, UserPointer, UserStruct};
use crate::kernel::task::{CapabilitySet, ProcessCapabilities, manager};
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

const PREFERRED_CAPABILITY_VERSION: u32 = 0x2008_0522;

#[derive(Clone, Copy)]
pub(super) enum Capability {
    NetRaw,
    SetPcap,
    SysTime,
}

#[derive(TryFromPrimitive)]
#[repr(u32)]
enum CapabilityVersion {
    V1 = 0x1998_0330,
    V2 = 0x2007_1026,
    V3 = PREFERRED_CAPABILITY_VERSION,
}

impl CapabilityVersion {
    fn data_entries(self) -> usize {
        match self {
            CapabilityVersion::V1 => 1,
            CapabilityVersion::V2 | CapabilityVersion::V3 => 2,
        }
    }
}

fn read_cap_user_header(header: UPtr<CapUserHeader>) -> SysResult<(CapUserHeader, CapabilityVersion)> {
    let mut header_value = header.read_optional()?.ok_or(Errno::EFAULT)?;
    let version = match CapabilityVersion::try_from(header_value.version) {
        Ok(version) => version,
        Err(_) => {
            header_value.version = PREFERRED_CAPABILITY_VERSION;
            header.write(header_value)?;
            return Err(Errno::EINVAL);
        }
    };
    Ok((header_value, version))
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CapUserHeader {
    version: u32,
    pid: i32,
}

impl UserStruct for CapUserHeader {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CapUserData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

impl UserStruct for CapUserData {}

impl CapUserData {
    const ZERO: Self = Self {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    };

    fn from_capabilities(capabilities: ProcessCapabilities) -> Self {
        Self {
            effective: capabilities.effective.bits(),
            permitted: capabilities.permitted.bits(),
            inheritable: capabilities.inheritable.bits(),
        }
    }

    fn into_capabilities(self, old: ProcessCapabilities) -> ProcessCapabilities {
        ProcessCapabilities {
            effective: CapabilitySet::from_bits_retain(self.effective),
            permitted: CapabilitySet::from_bits_retain(self.permitted),
            inheritable: CapabilitySet::from_bits_retain(self.inheritable),
            bounding: old.bounding,
        }
    }
}

pub(super) fn capable(capability: Capability) -> bool {
    let capability_set = match capability {
        Capability::NetRaw => CapabilitySet::NET_RAW,
        Capability::SetPcap => CapabilitySet::SETPCAP,
        Capability::SysTime => CapabilitySet::SYS_TIME,
    };
    current::pcb().capabilities().effective.contains(capability_set)
}

pub(super) fn drop_capability_from_bounding(capability: usize) -> SysResult<usize> {
    if !capable(Capability::SetPcap) {
        return Err(Errno::EPERM);
    }

    let capability = CapabilitySet::from_cap_number(capability).ok_or(Errno::EINVAL)?;
    let mut capabilities = current::pcb().capabilities();
    capabilities.bounding.remove(capability);
    current::pcb().set_capabilities(capabilities);
    Ok(0)
}

pub fn capget(header: UPtr<CapUserHeader>, data: UPtr<CapUserData>) -> SysResult<usize> {
    let (header, version) = read_cap_user_header(header)?;
    if header.pid < 0 {
        return Err(Errno::EINVAL);
    }
    let capabilities = if header.pid == 0 || header.pid == current::pid() {
        current::pcb().capabilities()
    } else {
        manager::get(header.pid).ok_or(Errno::ESRCH)?.parent().capabilities()
    };

    if !data.is_null() {
        data.write(CapUserData::from_capabilities(capabilities))?;
        for i in 1..version.data_entries() {
            data.add(i).write(CapUserData::ZERO)?;
        }
    }

    Ok(0)
}

pub fn capset(header: UPtr<CapUserHeader>, data: UPtr<CapUserData>) -> SysResult<usize> {
    let (header, version) = read_cap_user_header(header)?;

    if header.pid < 0 {
        return Err(Errno::EINVAL);
    }
    if header.pid != 0 && header.pid != current::pid() {
        if manager::get(header.pid).is_some() {
            return Err(Errno::EPERM);
        }
        return Err(Errno::ESRCH);
    }

    data.should_not_null()?;
    let old = current::pcb().capabilities();
    let capabilities = data.read()?.into_capabilities(old);
    for i in 1..version.data_entries() {
        data.add(i).read()?;
    }
    if !capabilities.permitted.contains(capabilities.effective) {
        return Err(Errno::EPERM);
    }
    if !old.permitted.contains(capabilities.permitted) {
        return Err(Errno::EPERM);
    }
    let allowed_inheritable = if old.effective.contains(CapabilitySet::SETPCAP) {
        old.inheritable | old.bounding
    } else {
        old.inheritable | old.permitted
    };
    if !allowed_inheritable.contains(capabilities.inheritable) {
        return Err(Errno::EPERM);
    }
    current::pcb().set_capabilities(capabilities);

    Ok(0)
}

pub fn getresuid(ruid: UPtr<Uid>, euid: UPtr<Uid>, suid: UPtr<Uid>) -> SysResult<usize> {
    let pcb = current::pcb();
    ruid.write(pcb.uid())?;
    euid.write(pcb.euid())?;
    suid.write(pcb.suid())?;
    Ok(0)
}

pub fn getresgid(rgid: UPtr<Uid>, egid: UPtr<Uid>, sgid: UPtr<Uid>) -> SysResult<usize> {
    let pcb = current::pcb();
    rgid.write(pcb.gid())?;
    egid.write(pcb.egid())?;
    sgid.write(pcb.sgid())?;
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
