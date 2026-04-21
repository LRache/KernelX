use num_enum::TryFromPrimitive;

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
pub enum Errno {
    EPERM = 1,            // Operation not permitted
    ENOENT = 2,           // No such file or directory
    ESRCH = 3,            // No such process
    EINTR = 4,            // Interrupted system call
    EIO = 5,              // Input/output error
    E2BIG = 7,            // Argument list too long
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
    ETXTBSY = 26,         // Text file busy
    EFBIG = 27,           // File too large
    ENOSPC = 28,          // No space left on device
    ESPIPE = 29,          // Illegal seek
    EROFS = 30,           // Read-only file system
    EPIPE = 32,           // Broken pipe
    ENAMETOOLONG = 36,    // File name too long
    ENOTEMPTY = 39,       // Directory not empty
    ELOOP = 40,           // Too many symbolic links encountered
    ENOSYS = 38,          // Function not implemented
    ENOTSOCK = 88,        // Socket operation on non-socket
    EDESTADDRREQ = 89,    // Destination address required
    EPROTOTYPE = 91,      // Protocol wrong type for socket
    EPROTONOSUPPORT = 93, // Protocol not supported
    EOPNOTSUPP = 95,      // Operation not supported on transport endpoint
    EAFNOSUPPORT = 97,    // Address family not supported by protocol
    EADDRINUSE = 98,      // Address already in use
    EADDRNOTAVAIL = 99,   // Cannot assign requested address
    ENETUNREACH = 101,    // Network is unreachable
    ECONNABORTED = 103,   // Software caused connection abort
    ECONNRESET = 104,     // Connection reset by peer
    ENOTCONN = 107,       // Transport endpoint is not connected
    ESHUTDOWN = 108,      // Cannot send after transport endpoint shutdown
    ETIMEDOUT = 110,      // Connection timed out
    ECONNREFUSED = 111,   // Connection refused
    EISCONN = 106,        // Transport endpoint is already connected
    EALREADY = 114,       // Operation already in progress
    EINPROGRESS = 115,    // Operation now in progress
}

pub type SysResult<T> = Result<T, Errno>;
