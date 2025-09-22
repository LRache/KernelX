#[derive(Eq, PartialEq, PartialOrd, Hash, Clone, Copy)]
pub struct Index {
    pub sno: u32,
    pub ino: u32,
}

impl Ord for Index {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        if self.sno != other.sno {
            self.sno.cmp(&other.sno)
        } else {
            self.ino.cmp(&other.ino)
        }
    }
}

impl Index {
    pub fn root() -> Self {
        Index { sno: 0, ino: 0 }
    }

    pub fn dontcare() -> Self {
        Index { sno: u32::MAX, ino: u32::MAX }
    }
}
