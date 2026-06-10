pub mod kernelpagetable;
mod pagetable;
mod pte;


pub use pagetable::PageTable;

#[cfg(feature = "kvm")]
mod sv39x4;
#[cfg(feature = "kvm")]
pub use sv39x4::Sv39x4PageTable;
