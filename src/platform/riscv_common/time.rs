use core::{arch::asm, ptr::read};

use crate::platform::config::TIMER_FREQ;

fn read_time() -> u64 {
    let time: u64;
    unsafe { asm!("csrr {}, time", out(reg) time); }
    time
}

pub fn get_time_us() -> u64 {
    read_time() * 1000000 as u64 / TIMER_FREQ as u64
}

pub fn set_next_timer_us(us: u64) {
    let next_time = read_time() + us * 1000000 as u64 / TIMER_FREQ as u64;
    unsafe { asm!("csrw stimecmp, {}", in(reg) next_time); }
}
