use std::os::fd::RawFd;

pub struct Fd(RawFd);

impl Fd {
    pub fn new(fd: RawFd) -> Self {
        Self(fd)
    }

    pub fn raw(&self) -> RawFd {
        self.0
    }
}

impl Drop for Fd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe {
                libc::close(self.0);
            }
            self.0 = -1;
        }
    }
}
