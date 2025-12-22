mod nofile;
mod swapper;
mod lru;
mod kswapd;
mod swappable;

pub use nofile::SwappableNoFileFrame;
pub use kswapd::spawn_kswapd;
pub use swapper::shrink;

use lru::LRUCache;
use swappable::SwappableFrame;

use alloc::collections::LinkedList;
use alloc::sync::{Arc, Weak};

use crate::kernel::mm::AddrSpace;
use crate::klib::SpinLock;

pub type AddrSpaceFamilyChain = Arc<SpinLock<LinkedList<Weak<AddrSpace>>>>;

use crate::driver::get_block_driver;

#[unsafe(link_section = ".text.init")]
pub fn init() {
    let driver = get_block_driver("virtio_mmio@10002000").expect("Swap driver not found");
    nofile::init_swapper(driver);
    swapper::init_swapper();
}

pub fn fini() {
    swapper::print_perf_info();
}
