use crate::kernel::errno::{Errno, SysResult};
use crate::klib::backtrace::print_backtrace;

pub fn debug_errno(msg: &str, errno: Errno) -> Errno {
    if errno == Errno::EIO {
        crate::kdebug!("ext4_native Err({:?}): {}", errno, msg);
        print_backtrace();
    }
    errno
}

pub fn ret_errno<T>(msg: &str, errno: Errno) -> SysResult<T> {
    Err(debug_errno(msg, errno))
}

pub fn mount_errno(msg: &str, errno: Errno) -> Errno {
    if matches!(errno, Errno::EOPNOTSUPP | Errno::EINVAL | Errno::EIO) {
        crate::kdebug!("ext4_native mount Err({:?}): {}", errno, msg);
        print_backtrace();
    }
    errno
}

pub fn mount_ret_errno<T>(msg: &str, errno: Errno) -> SysResult<T> {
    Err(mount_errno(msg, errno))
}

pub(super) fn get_slice(buf: &[u8], off: usize, len: usize) -> SysResult<&[u8]> {
    let end = off
        .checked_add(len)
        .ok_or_else(|| debug_errno("get_slice: offset + len overflow", Errno::EINVAL))?;
    buf.get(off..end)
        .ok_or_else(|| debug_errno("get_slice: range out of bounds", Errno::EIO))
}

pub(super) fn get_u8(buf: &[u8], off: usize) -> SysResult<u8> {
    buf.get(off)
        .copied()
        .ok_or_else(|| debug_errno("get_u8: offset out of bounds", Errno::EIO))
}

pub(super) fn get_u16_le(buf: &[u8], off: usize) -> SysResult<u16> {
    let s = get_slice(buf, off, 2)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

pub(super) fn get_u32_le(buf: &[u8], off: usize) -> SysResult<u32> {
    let s = get_slice(buf, off, 4)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub(super) fn put_u8(buf: &mut [u8], off: usize, val: u8) -> SysResult<()> {
    let b = buf
        .get_mut(off)
        .ok_or_else(|| debug_errno("put_u8: offset out of bounds", Errno::EIO))?;
    *b = val;
    Ok(())
}

pub(super) fn put_u16_le(buf: &mut [u8], off: usize, val: u16) -> SysResult<()> {
    let end = off
        .checked_add(2)
        .ok_or_else(|| debug_errno("put_u16_le: offset overflow", Errno::EINVAL))?;
    let s = buf
        .get_mut(off..end)
        .ok_or_else(|| debug_errno("put_u16_le: range out of bounds", Errno::EIO))?;
    s.copy_from_slice(&val.to_le_bytes());
    Ok(())
}

pub(super) fn put_u32_le(buf: &mut [u8], off: usize, val: u32) -> SysResult<()> {
    let end = off
        .checked_add(4)
        .ok_or_else(|| debug_errno("put_u32_le: offset overflow", Errno::EINVAL))?;
    let s = buf
        .get_mut(off..end)
        .ok_or_else(|| debug_errno("put_u32_le: range out of bounds", Errno::EIO))?;
    s.copy_from_slice(&val.to_le_bytes());
    Ok(())
}
