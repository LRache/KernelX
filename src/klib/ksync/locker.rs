pub trait LockerTrait {
    fn lock(&self, name: &'static str);
    fn unlock(&self, name: &'static str);
    fn try_lock(&self, name: &'static str) -> bool;
}
