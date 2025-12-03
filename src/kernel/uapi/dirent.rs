use crate::fs::FileType;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Dirent {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
}

pub enum DirentType {
    Unknown     = 0,
    FIFO        = 1,
    CharDevice  = 2,
    Directory   = 4,
    BlockDevice = 6,
    RegularFile = 8,
    Symlink     = 10,
    Socket      = 12,
}

impl From<FileType> for DirentType {
    fn from(ft: FileType) -> Self {
        match ft {
            FileType::Regular     => DirentType::RegularFile,
            FileType::Directory   => DirentType::Directory,
            FileType::CharDevice  => DirentType::CharDevice,
            FileType::BlockDevice => DirentType::BlockDevice,
            FileType::FIFO        => DirentType::FIFO,
            FileType::Symlink     => DirentType::Symlink,
            FileType::Socket      => DirentType::Socket,
            FileType::Unknown     => DirentType::Unknown,
        }
    }
}