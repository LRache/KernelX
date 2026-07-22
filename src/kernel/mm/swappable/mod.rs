#[cfg(all(feature = "swap-memory", not(feature = "no-smp"), target_arch = "loongarch64"))]
compile_error!("LoongArch swap-memory currently requires the no-smp feature for complete user-TLB invalidation");
#[cfg(all(feature = "swap-memory", feature = "kvm"))]
compile_error!("swap-memory cannot be combined with kvm until G-stage mappings participate in swap invalidation");

mod file;
mod kswapd;
mod nofile;
mod swappable;
mod swapper;

pub use file::{FileBackend, FileMapRegistration, FileMapping, FilePageIdentityPin, SharedFilePage};
pub use kswapd::spawn_kswapd;
pub use nofile::{AnonMapFamilyRegistration, AnonymousSwappableFrame, AnonymousSwappableFramePin};
#[cfg(feature = "swap-memory")]
pub(crate) use swappable::SwapError;
pub(crate) use swappable::{AccessDirty, ResidentPageGuard, SwappableFramePin, TlbInvalidationToken};
pub use swapper::{print_perf_info, shrink};

#[unsafe(link_section = ".text.init")]
pub fn init() {
    swapper::init_swapper();
}

#[cfg(feature = "swap-memory")]
#[unsafe(link_section = ".text.init")]
pub fn init_anonymous_swap(driver: Option<alloc::sync::Arc<dyn crate::driver::BlockDriverOps>>) {
    nofile::init_swap_space(driver);
}

pub fn fini() {
    swapper::print_perf_info();
}
