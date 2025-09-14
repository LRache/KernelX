mod task;
mod mm;
mod fs;
mod signal;
mod misc;
mod num;
mod def;

pub use num::syscall;

pub type Args = [usize; 7];
