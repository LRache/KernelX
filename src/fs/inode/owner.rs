use crate::kernel::uapi::Uid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner {
    pub uid: Uid,
    pub gid: Uid,
}

impl Owner {
    pub const fn new(uid: Uid, gid: Uid) -> Self {
        Self { uid, gid }
    }

    pub const fn root() -> Self {
        Self { uid: 0, gid: 0 }
    }
}
