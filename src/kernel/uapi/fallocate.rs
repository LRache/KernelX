use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FileFallocateFlags: usize {
        const FALLOC_FL_KEEP_SIZE = 0x01;
        const FALLOC_FL_PUNCH_HOLE = 0x02;
    }
}
