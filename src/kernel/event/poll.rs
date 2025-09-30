use bitflags::bitflags;

bitflags! {
    pub struct PollEventSet: i16 {
        const POLLIN   = 0x0001; // There is data to read.
        const POLLPRI  = 0x0002; // There is urgent data to read.
        const POLLOUT  = 0x0004; // Writing now will not block.
        const POLLERR  = 0x0008; // Error condition.
        const POLLHUP  = 0x0010; // Hung up.
        const POLLNVAL = 0x0020; // Invalid request: fd not open.
    }
}
