//! Early boot hooks for LoongArch.
//!
//! Phase 2 temporary: we don't yet have enough of the architecture
//! backend implemented to call all the way into `kernel::main::main`.
//! So `_entry` (in `clib/src/arch/loongarch/entry/entry.S`) will be
//! redirected here via a linker-visible C symbol named `main`, we print
//! a greeting through the UART MMIO window, and we park the CPU.
//!
//! Once Phase 2 milestones are met (DMW up, stack valid, BSS zeroed,
//! control flow reaches Rust) this file goes away and the real
//! `kernel::main::main` takes over.

/// ns16550a base address in the DMW0 (uncached) high half window.
/// Physical UART0 = 0x1fe0_01e0 (from `qemu-system-loongarch64 -M virt -machine dumpdtb=...`),
/// DMW0 = VSEG 0x8, MAT=SUC, PLV0.
const EARLY_UART_MMIO: usize = 0x8000_0000_1fe0_01e0;

/// Poke one byte at the UART transmit holding register. NS16550a is
/// "just write the byte at offset 0" — no need to poll LSR while QEMU
/// has an infinite-depth TX FIFO.
#[inline(always)]
unsafe fn uart_putc(c: u8) {
    unsafe { core::ptr::write_volatile(EARLY_UART_MMIO as *mut u8, c) };
}

#[inline(always)]
unsafe fn uart_puts(s: &str) {
    for &b in s.as_bytes() {
        unsafe { uart_putc(b) };
    }
}

/// Temporary `main` stand-in so we can close the loop on Phase 2 before
/// wiring up `Processor`, the scheduler, paging, etc.
///
/// Signature matches `kernel::main::main(hartid, heap_start, memory_top)`
/// so the asm in `entry.S` (which passes `a0`, `a1`, `a2`) doesn't care
/// whether this or the real one answers.
#[unsafe(no_mangle)]
pub extern "C" fn main(hartid: usize, heap_start: usize, memory_top: usize) -> ! {
    unsafe {
        uart_puts("\r\n[KernelX] Hello from LoongArch64!\r\n");
        uart_puts("[KernelX] Phase 2 entry probe\r\n");
        // Cheap hex dump of the three args so we can confirm entry.S
        // computed sensible values.
        uart_puts("  hartid      = ");
        uart_put_hex64(hartid as u64);
        uart_puts("\r\n  heap_start  = ");
        uart_put_hex64(heap_start as u64);
        uart_puts("\r\n  memory_top  = ");
        uart_put_hex64(memory_top as u64);
        uart_puts("\r\n[KernelX] parking.\r\n");
    }
    loop {
        unsafe { core::arch::asm!("idle 0", options(nomem, nostack)) };
    }
}

unsafe fn uart_put_hex64(mut v: u64) {
    unsafe { uart_puts("0x") };
    let mut buf = [0u8; 16];
    for i in (0..16).rev() {
        let nib = (v & 0xf) as u8;
        buf[i] = if nib < 10 { b'0' + nib } else { b'a' + (nib - 10) };
        v >>= 4;
    }
    for &c in &buf {
        unsafe { uart_putc(c) };
    }
}
