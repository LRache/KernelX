use crate::fs::vfs;

pub fn init() {
    vfs::vfs::VFS.init();
}
