#[allow(dead_code)]
#[derive(Debug)]
pub struct TaskCloneFlags {
    pub files: bool,
    pub vm: bool,
    pub thread: bool,
    pub parent: bool,
    pub vfork: bool,
}
