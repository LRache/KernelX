#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Clone, Copy, Debug)]
pub struct Index {
    pub sno: u32,
    pub ino: u32,
}
