use core::ops::{Deref, DerefMut};

use crate::arch::PGSIZE;

use super::area::PinPageFrame;

pub struct ReadChunk {
    frame: PinPageFrame,
    offset: usize,
    length: usize,
}

impl ReadChunk {
    pub fn new(frame: PinPageFrame, offset: usize, length: usize) -> Self {
        debug_assert!(offset <= PGSIZE && length <= PGSIZE - offset);
        Self { frame, offset, length }
    }
}

impl Deref for ReadChunk {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        // SAFETY: `frame` keeps the physical page alive and resident until the
        // chunk is dropped, and the constructor confines the range to one page.
        unsafe { core::slice::from_raw_parts((self.frame.kpage() + self.offset) as *const u8, self.length) }
    }
}

pub struct WriteChunk {
    frame: PinPageFrame,
    offset: usize,
    length: usize,
}

impl WriteChunk {
    pub(crate) fn new(frame: PinPageFrame, offset: usize, length: usize) -> Self {
        debug_assert!(offset <= PGSIZE && length <= PGSIZE - offset);
        Self { frame, offset, length }
    }
}

impl Deref for WriteChunk {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        // SAFETY: See `ReadChunk::deref`.
        unsafe { core::slice::from_raw_parts((self.frame.kpage() + self.offset) as *const u8, self.length) }
    }
}

impl DerefMut for WriteChunk {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The chunk was created through `translate_write`, and the
        // returned slice is confined to the pinned page.
        unsafe { core::slice::from_raw_parts_mut((self.frame.kpage() + self.offset) as *mut u8, self.length) }
    }
}
