use core::time::Duration;

use super::shrink;
use crate::kernel::mm::page;
use crate::kernel::scheduler::current;

pub fn kswapd() {
    loop {
        // sleep 0.5s
        current::sleep(Duration::from_millis(500));
        if !page::need_to_shrink() {
            continue;
        }
        shrink(1024, 0);
    }
}
