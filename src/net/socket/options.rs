/// Socket option levels
pub const SOL_SOCKET: usize = 1;

/// Socket options (SOL_SOCKET level)
pub const SO_REUSEADDR: usize = 2;
pub const SO_ERROR: usize = 4;
pub const SO_KEEPALIVE: usize = 9;
pub const SO_RCVBUF: usize = 8;
pub const SO_SNDBUF: usize = 7;
pub const SO_RCVTIMEO: usize = 20;
pub const SO_SNDTIMEO: usize = 21;

/// IP-level options
pub const IPPROTO_IP: usize = 0;
pub const IPPROTO_TCP: usize = 6;

/// TCP options
pub const TCP_NODELAY: usize = 1;
