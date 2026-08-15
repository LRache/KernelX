use crate::kernel::errno::{Errno, SysResult};

pub mod inode;

pub fn decode_result(result: isize, max: usize) -> SysResult<usize> {
    if result < 0 {
        let errno = result
            .checked_neg()
            .and_then(|errno| i32::try_from(errno).ok())
            .and_then(|errno| Errno::try_from(errno).ok())
            .unwrap_or(Errno::EIO);
        return Err(errno);
    }

    let result = result as usize;
    if result > max {
        return Err(Errno::EIO);
    }
    Ok(result)
}
