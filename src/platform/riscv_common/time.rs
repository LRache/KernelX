use crate::platform::config::TIMER_FREQ;

fn read_time() -> u64 {
    let time: u64;
    unsafe { core::arch::asm!("csrr {}, time", out(reg) time); }
    time
}

pub fn get_time_us() -> u64 {
    read_time() * 1000000 as u64 / TIMER_FREQ as u64
}

#[allow(dead_code)]
pub fn clear_timer_interrupt() {
    let t = u64::MAX;
    unsafe { core::arch::asm!("csrw stimecmp, {}", in(reg) t); }
}

pub fn set_next_timer_us(us: u64) {
    let next_time = read_time() + us * 1000000 as u64 / TIMER_FREQ as u64;
    unsafe { core::arch::asm!("csrw stimecmp, {}", in(reg) next_time); }
}
