mod anonymous;
mod lru;

pub use anonymous::AnonymousFrame;

use lru::LRUCache;

use alloc::collections::LinkedList;
use alloc::sync::{Arc, Weak};

use crate::kernel::mm::AddrSpace;
use crate::klib::SpinLock;

pub type AddrSpaceFamilyChain = Arc<SpinLock<LinkedList<Weak<AddrSpace>>>>;

use crate::driver::get_block_driver;

#[unsafe(link_section = ".text.init")]
pub fn init() {
    let driver = get_block_driver("virtio_block0").expect("Swap driver not found");
    anonymous::init_swapper(driver);
}

pub fn fini() {
    anonymous::print_perf_info();
}

pub fn shrink(mut page_count: usize) {
    anonymous::shrink(&mut page_count);
}
