#[cfg(feature = "kvm")]
mod kvm;
mod loop_dev;
mod null;
mod rtc;
mod urandom;
mod zero;

#[cfg(feature = "kvm")]
pub use kvm::KvmInode;
pub use loop_dev::LoopInode;
pub use null::NullInode;
pub use rtc::RtcInode;
pub use urandom::URandomInode;
pub use zero::ZeroInode;
