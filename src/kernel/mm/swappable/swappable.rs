pub trait SwappableFrame {
    fn swap_out(&self, dirty: bool) -> bool;
    fn take_access_dirty_bit(&self) -> Option<(bool, bool)>;
}
