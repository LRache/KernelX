use crate::arch;
use crate::{println, print};

#[cfg(debug_assertions)]
#[inline(never)]
pub unsafe fn print_backtrace() {
    let mut fp = crate::arch::get_frame_pointer() as *const usize;

    crate::println!("\n--- Stack Backtrace (Debug) ---");

    let mut depth = 0usize;
    while !fp.is_null() && depth < 64 {
        if fp as usize % core::mem::size_of::<usize>() != 0 {
            break;
        }

        let (ra, prev_fp_addr) = unsafe { arch::frame_info(fp as usize) };
        let prev_fp = prev_fp_addr as *const usize;

        print!("[{:02}]{:#018x}: ", depth, ra);
        if let Some((name, off)) = crate::klib::symbol::lookup(ra) {
            print!("{}+{:#x}", name, off);
        } else {
            print!("???");
        }
        println!();

        if prev_fp <= fp {
            break;
        }

        if !arch::is_kernel_addr(prev_fp as usize) {
            break;
        }

        fp = prev_fp;
        depth += 1;
    }

    println!("--- End of Backtrace ---\n");
}

#[cfg(not(debug_assertions))]
#[inline(always)]
pub fn print_backtrace() {}
