#[cfg(feature = "platform-riscv-common")]
mod riscv_common;

#[cfg(feature = "platform-qemu-virt-riscv64")]
mod qemu_virt_riscv64;

#[cfg(feature = "platform-qemu-virt-riscv64")]
pub use qemu_virt_riscv64::*;
