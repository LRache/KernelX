use num_enum::TryFromPrimitive;

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
pub enum Errno {
    EPERM   =  1,  // Operation not permitted
    ENOENT  =  2,  // No such file or directory
    EIO     =  5,  // Input/output error
    ENOEXEC =  8,  // Exec format error
    EBADF   =  9,  // Bad file descriptor
    ECHILD  = 10,  // No child processes
    ENOMEM  = 12,  // Out of memory
    EFAULT  = 14,  // Bad address
    EEXIST  = 17,  // File exists
    EXDEV   = 18,  // Cross-device link
    ENOTDIR = 20,  // Not a directory
    EINVAL  = 22,  // Invalid argument
    EFBIG   = 27,  // File too large
    ENOSPC  = 28,  // No space left on device
    ESPIPE  = 29,  // Illegal seek
    EPIPE   = 32,  // Broken pipe
    ENOSYS  = 38,  // Function not implemented
    EOPNOTSUPP = 95, // Operation not supported on transport endpoint
}

pub type SysResult<T> = Result<T, Errno>;
