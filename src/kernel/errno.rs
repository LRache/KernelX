use num_enum::TryFromPrimitive;

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
pub enum Errno {
    EPERM = 1,            // Operation not permitted
    ENOENT = 2,           // No such file or directory
    ESRCH = 3,            // No such process
    EINTR = 4,            // Interrupted system call
    EIO = 5,              // Input/output error
    ENOEXEC = 8,          // Exec format error
    EBADF = 9,            // Bad file descriptor
    ECHILD = 10,          // No child processes
    EAGAIN = 11,          // Try again
    ENOMEM = 12,          // Out of memory
    EACCES = 13,          // Permission denied
    EFAULT = 14,          // Bad address
    EEXIST = 17,          // File exists
    EXDEV = 18,           // Cross-device link
    ENODEV = 19,          // No such device
    ENOTDIR = 20,         // Not a directory
    EISDIR = 21,          // Is a directory
    EINVAL = 22,          // Invalid argument
    EMFILE = 24,          // Too many open files
    ENOTTY = 25,          // Not a typewriter (inappropriate ioctl)
    EFBIG = 27,           // File too large
    ENOSPC = 28,          // No space left on device
    ESPIPE = 29,          // Illegal seek
    EROFS = 30,           // Read-only file system
    EPIPE = 32,           // Broken pipe
    ENOTEMPTY = 39,       // Directory not empty
    ENOSYS = 38,          // Function not implemented
    EAFNOSUPPORT = 97,    // Address family not supported by protocol
    EOPNOTSUPP = 95,      // Operation not supported on transport endpoint
    EPROTONOSUPPORT = 93, // Protocol not supported
    ETIMEDOUT = 110,      // Connection timed out
}

pub type SysResult<T> = Result<T, Errno>;
