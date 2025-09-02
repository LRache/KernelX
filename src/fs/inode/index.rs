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
