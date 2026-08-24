use virtio_drivers::transport::pci::bus::{Cam, DeviceFunction};

use crate::arch;

fn offset(cam: Cam, df: DeviceFunction, register_offset: u16) -> usize {
    let bdf = ((df.bus as usize) << 8) | ((df.device as usize) << 3) | df.function as usize;
    let config_offset = match cam {
        Cam::MmioCam => bdf << 8,
        Cam::Ecam => bdf << 12,
    };
    config_offset + register_offset as usize
}

pub(super) fn read_u8(base: usize, cam: Cam, df: DeviceFunction, register_offset: u16) -> u8 {
    let ptr = (base + offset(cam, df, register_offset)) as *const u8;
    // SAFETY: `base` is a mapped PCI configuration-space window, and `df` and
    // `register_offset` identify a byte within the enumerated function.
    unsafe { arch::read_volatile(ptr) }
}

pub(super) fn read_u16(base: usize, cam: Cam, df: DeviceFunction, register_offset: u16) -> u16 {
    let ptr = (base + offset(cam, df, register_offset)) as *const u16;
    // SAFETY: `base` is a mapped PCI configuration-space window. PCI u16
    // registers are naturally aligned and belong to the enumerated function.
    unsafe { arch::read_volatile(ptr) }
}

pub(in crate::driver) fn read_u32(base: usize, cam: Cam, df: DeviceFunction, register_offset: u16) -> u32 {
    let ptr = (base + offset(cam, df, register_offset)) as *const u32;
    // SAFETY: `base` is a mapped PCI configuration-space window. PCI u32
    // registers are naturally aligned and belong to the enumerated function.
    unsafe { arch::read_volatile(ptr) }
}

pub(super) fn write_u16(base: usize, cam: Cam, df: DeviceFunction, register_offset: u16, value: u16) {
    let ptr = (base + offset(cam, df, register_offset)) as *mut u16;
    // SAFETY: `base` is a mapped PCI configuration-space window. PCI u16
    // registers are naturally aligned and belong to the enumerated function.
    unsafe { arch::write_volatile(ptr, value) }
}

pub(super) fn write_u32(base: usize, cam: Cam, df: DeviceFunction, register_offset: u16, value: u32) {
    let ptr = (base + offset(cam, df, register_offset)) as *mut u32;
    // SAFETY: `base` is a mapped PCI configuration-space window. PCI u32
    // registers are naturally aligned and belong to the enumerated function.
    unsafe { arch::write_volatile(ptr, value) }
}
