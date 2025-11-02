use spin::Lazy;

use crate::arch;

use super::SpinLock;

static STATE: Lazy<SpinLock<u32>> = Lazy::new(|| {
    let time = arch::get_time_us() as u32;
    SpinLock::new(time ^ 0xDEECE66D)
});

pub fn random() -> u32 {
    // A very simple linear congruential generator (LCG)
    let mut state = STATE.lock();
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state
}
