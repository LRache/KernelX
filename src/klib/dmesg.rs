use crate::klib::SpinLock;

/// dmesg 环形缓冲区大小: 64KB
const DMESG_BUF_SIZE: usize = 64 * 1024;

struct DmesgBuffer {
    buf: [u8; DMESG_BUF_SIZE],
    head: usize,
    len: usize,
}

impl DmesgBuffer {
    const fn new() -> Self {
        DmesgBuffer {
            buf: [0u8; DMESG_BUF_SIZE],
            head: 0,
            len: 0,
        }
    }

    fn write(&mut self, data: &[u8]) {
        for &byte in data {
            self.buf[self.head] = byte;
            self.head = (self.head + 1) % DMESG_BUF_SIZE;
            if self.len < DMESG_BUF_SIZE {
                self.len += 1;
            }
        }
    }

    fn read(&self, dest: &mut [u8]) -> usize {
        let to_copy = self.len.min(dest.len());
        if to_copy == 0 {
            return 0;
        }

        let start = if self.len < DMESG_BUF_SIZE { 0 } else { self.head };

        for i in 0..to_copy {
            dest[i] = self.buf[(start + i) % DMESG_BUF_SIZE];
        }

        to_copy
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    fn len(&self) -> usize {
        self.len
    }

    fn capacity(&self) -> usize {
        DMESG_BUF_SIZE
    }
}

static DMESG: SpinLock<DmesgBuffer> = SpinLock::new(DmesgBuffer::new(), "dmesg");

pub fn dmesg_write(s: &str) {
    let mut buf = DMESG.lock();
    buf.write(s.as_bytes());
}

pub fn dmesg_read(dest: &mut [u8]) -> usize {
    let buf = DMESG.lock();
    buf.read(dest)
}

pub fn dmesg_read_clear(dest: &mut [u8]) -> usize {
    let mut buf = DMESG.lock();
    let n = buf.read(dest);
    buf.clear();
    n
}

pub fn dmesg_clear() {
    let mut buf = DMESG.lock();
    buf.clear();
}

pub fn dmesg_size_unread() -> usize {
    let buf = DMESG.lock();
    buf.len()
}

pub fn dmesg_size_buffer() -> usize {
    let buf = DMESG.lock();
    buf.capacity()
}
