use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Mode: u16 {
        const S_IFMT   = 0o170000; // bit mask for the file type bit field

        const S_IFSOCK = 0o140000; // socket
        const S_IFLNK  = 0o120000; // symbolic link
        const S_IFREG  = 0o100000; // regular file
        const S_IFBLK  = 0o060000; // block device
        const S_IFDIR  = 0o040000; // directory
        const S_IFCHR  = 0o020000; // character device
        const S_IFIFO  = 0o010000; // FIFO

        const S_ISUID  = 0o4000;   // set-user-ID bit
        const S_ISGID  = 0o2000;   // set-group-ID bit
        const S_ISVTX  = 0o1000;   // sticky bit

        const S_IRUSR  = 0o0400;   // owner has read permission
        const S_IWUSR  = 0o0200;   // owner has write permission
        const S_IXUSR  = 0o0100;   // owner has execute permission

        const S_IRGRP  = 0o0040;   // group has read permission
        const S_IWGRP  = 0o0020;   // group has write permission
        const S_IXGRP  = 0o0010;   // group has execute permission

        const S_IROTH  = 0o0004;   // others have read permission
        const S_IWOTH  = 0o0002;   // others have write permission
        const S_IXOTH  = 0o0001;   // others have execute permission
    }
}


#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    FIFO,
    Socket,
    Unknown,
}

impl Into<FileType> for Mode {
    fn into(self) -> FileType {
        match self & Mode::S_IFMT {
            Mode::S_IFREG  => FileType::Regular,
            Mode::S_IFDIR  => FileType::Directory,
            Mode::S_IFLNK  => FileType::Symlink,
            Mode::S_IFCHR  => FileType::CharDevice,
            Mode::S_IFBLK  => FileType::BlockDevice,
            Mode::S_IFIFO  => FileType::FIFO,
            Mode::S_IFSOCK => FileType::Socket,
            _              => FileType::Unknown,
        }
    }
}
