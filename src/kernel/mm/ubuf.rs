use core::fmt::Debug;

use crate::arch;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::AddrSpace;
use crate::kernel::mm::maparea::{ReadChunk, WriteChunk};

pub struct UAddrSpaceBuffer<'a> {
    uaddr: usize,
    length: usize,
    addrspace: &'a AddrSpace,
}

impl<'a> UAddrSpaceBuffer<'a> {
    pub fn new(uaddr: usize, length: usize, addrspace: &'a AddrSpace) -> Self {
        Self {
            uaddr,
            length,
            addrspace,
        }
    }

    pub fn write(&self, offset: usize, buf: &[u8]) -> SysResult<()> {
        debug_assert!(offset + buf.len() <= self.length);
        self.addrspace.copy_to_user_buffer(self.uaddr + offset, buf)
    }

    pub fn length(&self) -> usize {
        self.length
    }

    pub fn with_length(&self, length: usize) -> Self {
        Self {
            uaddr: self.uaddr,
            length,
            addrspace: self.addrspace,
        }
    }

    pub fn uaddr(&self) -> usize {
        self.uaddr
    }

    pub fn iter(&'a self) -> ReaderIter<'a> {
        ReaderIter { ubuf: self, offset: 0 }
    }

    pub fn iter_mut(&'a self) -> WriterIter<'a> {
        WriterIter { ubuf: self, offset: 0 }
    }
}

pub struct ReaderIter<'a> {
    ubuf: &'a UAddrSpaceBuffer<'a>,
    offset: usize,
}

impl<'a> Iterator for ReaderIter<'a> {
    type Item = SysResult<ReadChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.ubuf.length - self.offset;
        if remaining == 0 {
            return None;
        }

        let uaddr = match self.ubuf.uaddr.checked_add(self.offset) {
            Some(uaddr) => uaddr,
            None => return Some(Err(Errno::EFAULT)),
        };
        let length = core::cmp::min(remaining, arch::PGSIZE - (uaddr & arch::PGMASK));
        self.offset += length;
        Some(self.ubuf.addrspace.translate_read(uaddr, length))
    }
}

pub struct WriterIter<'a> {
    ubuf: &'a UAddrSpaceBuffer<'a>,
    offset: usize,
}

impl<'a> Iterator for WriterIter<'a> {
    type Item = SysResult<WriteChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.ubuf.length - self.offset;
        if remaining == 0 {
            return None;
        }

        let uaddr = match self.ubuf.uaddr.checked_add(self.offset) {
            Some(uaddr) => uaddr,
            None => return Some(Err(Errno::EFAULT)),
        };
        let length = core::cmp::min(remaining, arch::PGSIZE - (uaddr & arch::PGMASK));
        self.offset += length;
        Some(self.ubuf.addrspace.translate_write(uaddr, length))
    }
}

impl Debug for UAddrSpaceBuffer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UAddrSpaceBuffer")
            .field("uaddr", &self.uaddr)
            .field("length", &self.length)
            .finish()
    }
}
