use crate::kernel::errno::Errno;

pub fn rt_sigprocmask(_how: usize, _set: usize, _oldset: usize) -> Result<usize, Errno> {
    Ok(0)
}
