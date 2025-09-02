use super::VFS;

pub fn init() {
    unsafe {
        VFS.init();
    }
}
