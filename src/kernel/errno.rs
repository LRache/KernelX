#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    ENOENT  =  2,  // No such file or directory
    EIO     =  5,  // Input/output error
    ENOEXEC =  8,  // Exec format error
    EBADFD  =  9,  // Bad file descriptor
    ECHILD  = 10,  // No child processes
    ENOMEM  = 12,  // Out of memory
    EFAULT  = 14,  // Bad address
    EEXIST  = 17,  // File exists
    EINVAL  = 22,  // Invalid argument
    ENOSYS  = 38,  // Function not implemented
    EOPNOTSUPP = 95, // Operation not supported on transport endpoint
}

impl From<i32> for Errno {
    fn from(value: i32) -> Self {
        match value {
            2  => Errno::ENOENT,
            5  => Errno::EIO,
            8  => Errno::ENOEXEC,
            9  => Errno::EBADFD, 
            10 => Errno::ECHILD,
            14 => Errno::EFAULT,
            22 => Errno::EINVAL,
            38 => Errno::ENOSYS,
            95 => Errno::EOPNOTSUPP,
            _ => panic!("Unknown errno value: {}", value),
        }
    }
}

pub type SysResult<T> = Result<T, Errno>;
