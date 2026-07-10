use crate::kernel::errno::{Errno, SysResult};

pub fn should_not_be_negative(value: usize) -> SysResult<usize> {
    if (value as isize) < 0 {
        Err(Errno::EINVAL)
    } else {
        Ok(value as usize)
    }
}
