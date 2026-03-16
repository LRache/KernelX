#[cfg(feature = "backtrace")]
#[inline(never)]
pub fn print_backtrace() {
    use crate::arch;
    use crate::{println, print};
    
    let mut fp = crate::arch::get_frame_pointer() as *const usize;

    crate::println!("\n--- Stack Backtrace ---");

    let mut depth = 0usize;
    while !fp.is_null() && depth < 64 {
        if fp as usize % core::mem::size_of::<usize>() != 0 {
            break;
        }

        if !arch::is_kernel_addr(fp as usize) {
            break;
        }

        let (ra, prev_fp_addr) = unsafe { arch::frame_info(fp as usize) };
        let prev_fp = prev_fp_addr as *const usize;

        if !arch::is_kernel_addr(ra as usize) {
            break;
        }

        print!("[{:02}] {:#018x}: ", depth, ra);
        if let Some((name, off)) = crate::klib::symbol::lookup(ra) {
            print!("{}+{:#x}", name, off);
        } else {
            print!("???");
        }
        println!();

        if prev_fp <= fp {
            break;
        }

        fp = prev_fp;
        depth += 1;
    }

    println!("--- End of Backtrace ---\n");
}

#[cfg(not(feature = "backtrace"))]
#[inline(always)]
pub fn print_backtrace() {}
