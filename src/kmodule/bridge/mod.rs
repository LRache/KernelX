use alloc::string::String;
use core::ffi::c_void;

use crate::fs::devfs;
use crate::kernel::errno::{Errno, SysResult};
use crate::kmodule::exports::kmodule_export;

pub mod inode;

use inode::{BridgeInode, BridgeInodeOps};

/// Registers a bridge inode directly below the devfs root.
///
/// The inode's `type_name` is used as its devfs entry name. On success, this
/// returns the registered inode number; on failure, it returns a negative
/// errno.
///
/// # Safety
///
/// `inode` must point to an initialized [`BridgeInodeOps`] that remains valid
/// for the lifetime of the registered devfs inode. The pointer itself is passed
/// to every callback as `data`, so any containing object and callback state
/// must remain alive and synchronize concurrent access for the same lifetime.
#[kmodule_export]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn devfs_register(inode: *mut BridgeInodeOps) -> isize {
    // SAFETY: The caller guarantees that inode is either null or points to an
    // initialized BridgeInodeOps with the lifetime documented above.
    let Some(ops) = (unsafe { inode.as_ref() }).copied() else {
        return -(Errno::EINVAL as isize);
    };

    let data = inode.cast::<c_void>();
    // SAFETY: The caller guarantees that the type_name callback, data pointer,
    // and returned string remain valid for the registered inode's lifetime.
    let name = String::from(unsafe { ops.decode_type_name(data) });

    match devfs::add_inode(name, |ino| {
        // SAFETY: The caller guarantees that the callback table, the pointer
        // used as callback data, and all state reachable through it remain
        // valid and synchronized for the lifetime of the registered inode.
        unsafe { BridgeInode::new(ino, ops, data) }
    }) {
        Ok(ino) => ino as isize,
        Err(errno) => -(errno as isize),
    }
}

pub fn decode_result(result: isize) -> SysResult<usize> {
    if result < 0 {
        let errno = result
            .checked_neg()
            .and_then(|errno| i32::try_from(errno).ok())
            .and_then(|errno| Errno::try_from(errno).ok())
            .unwrap_or(Errno::EIO);
        return Err(errno);
    }

    Ok(result as usize)
}
