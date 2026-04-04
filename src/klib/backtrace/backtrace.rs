use alloc::collections::LinkedList;

use crate::klib::backtrace::ksymbol;
use crate::{arch, print, println};

#[inline(always)]
fn next_frame(fp: *const usize) -> Option<(usize, *const usize)> {
    if fp.is_null() || (fp as usize) % core::mem::size_of::<usize>() != 0 {
        return None;
    }

    let (return_address, prev_fp_addr) = unsafe { arch::frame_info(fp as usize) };
    if !arch::is_kernel_addr(return_address as usize) {
        return None;
    }

    Some((return_address, prev_fp_addr as *const usize))
}

fn for_each_frame(mut fp: *const usize, mut f: impl FnMut(usize, usize)) {
    let mut depth = 0;
    while !fp.is_null() && depth < 64 {
        let (return_address, prev_fp) = match next_frame(fp) {
            Some(info) => info,
            None => break,
        };

        f(depth, return_address);

        if prev_fp <= fp {
            break;
        }

        fp = prev_fp;
        depth += 1;
    }
}

#[inline(always)]
pub fn backtrace_chain() -> LinkedList<usize> {
    let fp = arch::get_frame_pointer() as *const usize;
    let mut chain = LinkedList::new();

    for_each_frame(fp, |_, addr| chain.push_back(addr));

    chain
}

fn print_frame(depth: usize, addr: usize) {
    print!("[{}] {:#018x}: ", depth, addr);
    if let Some((name, off)) = ksymbol::lookup(addr) {
        print!("{}+{:#x}", name, off);
    } else {
        print!("???");
    }
    println!();
}

#[inline(never)]
pub fn print_backtrace_chain(chain: &LinkedList<usize>) {
    println!("\n--- Stack Backtrace ---");

    chain
        .iter()
        .enumerate()
        .for_each(|(depth, addr)| print_frame(depth, *addr));

    println!("--- End of Backtrace ---\n");
}

#[inline(never)]
pub fn print_backtrace() {
    let fp = arch::get_frame_pointer();
    print_backtrace_from_fp(fp);
}

pub fn print_backtrace_from_fp(fp: usize) {
    println!("\n--- Stack Backtrace ---");

    for_each_frame(fp as *const usize, print_frame);

    println!("--- End of Backtrace ---\n");
}
