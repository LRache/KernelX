use core::time::Duration;

use crate::kernel::{mm::page, scheduler::current};
use super::shrink;

fn kswapd() {
    loop {
        // sleep 0.5s
        current::sleep(Duration::from_millis(500));
        if !page::need_to_shrink() {
            continue;
        }
        shrink(1024, 0);
    }
}

pub fn spawn_kswapd() {
    crate::kernel::kthread::spawn(kswapd);
}
