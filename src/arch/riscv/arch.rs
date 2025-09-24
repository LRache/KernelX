use core::arch::asm;

pub fn wait_for_interrupt() {
    unsafe { asm!("wfi") };
}
