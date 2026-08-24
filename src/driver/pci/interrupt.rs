use alloc::collections::BTreeMap;
use core::mem::size_of;
use core::ptr::addr_of_mut;

use bitflags::bitflags;
use fdt::node::FdtNode;
use virtio_drivers::transport::pci::bus::{BarInfo, Cam, DeviceFunction, PciRoot};

use crate::driver::PciInterrupt;
use crate::{arch, kinfo, kwarn};

use super::config;

#[repr(u8)]
enum PciCapabilityId {
    Msi = 0x05,
    Msix = 0x11,
}

mod msi_control {
    pub const ENABLE: u16 = 1 << 0;
    pub const MULTIPLE_MESSAGE_ENABLE_MASK: u16 = 0x7 << 4;
    pub const ADDRESS_64BIT: u16 = 1 << 7;
    pub const PER_VECTOR_MASKING: u16 = 1 << 8;
}

pub(super) struct MsiAllocator {
    doorbell: u64,
    next_irq: u32,
    end_irq: u32,
}

impl MsiAllocator {
    pub(super) fn new(doorbell: u64, first_irq: u32, end_irq: u32) -> Self {
        Self {
            doorbell,
            next_irq: first_irq,
            end_irq,
        }
    }

    fn alloc(&mut self) -> Option<MsiMessage> {
        if self.next_irq >= self.end_irq {
            return None;
        }
        let irq = self.next_irq;
        self.next_irq += 1;
        Some(MsiMessage {
            address: self.doorbell,
            data: irq as u16,
            irq,
        })
    }
}

struct MsiMessage {
    address: u64,
    data: u16,
    irq: u32,
}

bitflags! {
    struct MsixMessageControl: u16 {
        const FUNCTION_MASK = 1 << 14;
        const ENABLE = 1 << 15;
    }
}

bitflags! {
    struct MsixVectorControl: u32 {
        const MASKED = 1 << 0;
    }
}

#[repr(C)]
struct MsixTableEntry {
    message_address_low: u32,
    message_address_high: u32,
    message_data: u32,
    vector_control: u32,
}

pub(super) fn enable_msix(
    config_kaddr: usize,
    cam: Cam,
    root: &PciRoot,
    bars: &[Option<BarInfo>; 6],
    df: DeviceFunction,
    msi: &mut MsiAllocator,
) -> Option<PciInterrupt> {
    let capability = root
        .capabilities(df)
        .find(|capability| capability.id == PciCapabilityId::Msix as u8)?;
    let control = config::read_u16(config_kaddr, cam, df, capability.offset as u16 + 2);
    let table_entries = (control & 0x07ff) as usize + 1;
    let table_info = config::read_u32(config_kaddr, cam, df, capability.offset as u16 + 4);
    let table_bar = (table_info & 0x7) as u8;
    if table_bar >= 6 {
        kwarn!("pci: {} has invalid MSI-X table BAR {}", df, table_bar);
        return None;
    }

    let table_offset = (table_info & !0x7) as u64;
    let table_bytes = table_entries.checked_mul(size_of::<MsixTableEntry>())? as u64;
    let (bar_address, bar_size) = bars[table_bar as usize].as_ref()?.memory_address_size()?;
    if table_offset.checked_add(table_bytes)? > bar_size as u64 {
        kwarn!(
            "pci: {} MSI-X table offset {:#x} size {:#x} exceeds BAR{} size {:#x}",
            df,
            table_offset,
            table_bytes,
            table_bar,
            bar_size,
        );
        return None;
    }

    let message = msi.alloc()?;
    let vector = 0u16;
    let table_pa = bar_address.checked_add(table_offset)?;
    let table_kaddr = arch::mmio_phys_to_kaddr(table_pa as usize, size_of::<MsixTableEntry>());
    let entry = table_kaddr as *mut MsixTableEntry;

    let masked_control = control | MsixMessageControl::FUNCTION_MASK.bits();
    config::write_u16(config_kaddr, cam, df, capability.offset as u16 + 2, masked_control);
    // SAFETY: the selected BAR contains the complete MSI-X table entry, and
    // `table_kaddr` maps that MMIO entry. Writes follow the PCI MSI-X masking
    // sequence so the device cannot observe a partially initialized vector.
    unsafe {
        arch::write_volatile(addr_of_mut!((*entry).vector_control), MsixVectorControl::MASKED.bits());
        arch::write_volatile(addr_of_mut!((*entry).message_address_low), message.address as u32);
        arch::write_volatile(
            addr_of_mut!((*entry).message_address_high),
            (message.address >> 32) as u32,
        );
        arch::write_volatile(addr_of_mut!((*entry).message_data), message.data as u32);
        arch::write_volatile(addr_of_mut!((*entry).vector_control), MsixVectorControl::empty().bits());
    }

    let enabled_control = (control | MsixMessageControl::ENABLE.bits()) & !MsixMessageControl::FUNCTION_MASK.bits();
    config::write_u16(config_kaddr, cam, df, capability.offset as u16 + 2, enabled_control);

    kinfo!("pci: {} uses MSI-X IRQ {} vector {}", df, message.irq, vector);
    Some(PciInterrupt::Msix {
        irq: message.irq,
        vector,
    })
}

pub(super) fn enable_msi(
    config_kaddr: usize,
    cam: Cam,
    root: &PciRoot,
    df: DeviceFunction,
    msi: &mut MsiAllocator,
) -> Option<u32> {
    let capability = root
        .capabilities(df)
        .find(|capability| capability.id == PciCapabilityId::Msi as u8)?;
    let message = msi.alloc()?;
    let control = config::read_u16(config_kaddr, cam, df, capability.offset as u16 + 2);
    let is_64bit = control & msi_control::ADDRESS_64BIT != 0;
    if !is_64bit && message.address > u32::MAX as u64 {
        kwarn!(
            "pci: {} MSI doorbell {:#x} does not fit in 32-bit message address",
            df,
            message.address,
        );
        return None;
    }

    config::write_u32(
        config_kaddr,
        cam,
        df,
        capability.offset as u16 + 4,
        message.address as u32,
    );
    let data_offset = if is_64bit {
        config::write_u32(
            config_kaddr,
            cam,
            df,
            capability.offset as u16 + 8,
            (message.address >> 32) as u32,
        );
        capability.offset as u16 + 12
    } else {
        capability.offset as u16 + 8
    };
    config::write_u16(config_kaddr, cam, df, data_offset, message.data);

    if control & msi_control::PER_VECTOR_MASKING != 0 {
        config::write_u32(config_kaddr, cam, df, data_offset + 4, 0);
    }

    let new_control = (control & !msi_control::MULTIPLE_MESSAGE_ENABLE_MASK) | msi_control::ENABLE;
    config::write_u16(config_kaddr, cam, df, capability.offset as u16 + 2, new_control);

    kinfo!("pci: {} uses MSI IRQ {}", df, message.irq);
    Some(message.irq)
}

pub(super) struct IntxTable {
    entries: BTreeMap<(u32, u32), u32>,
}

impl IntxTable {
    pub(super) fn from_fdt(node: &FdtNode) -> Self {
        let mut entries = BTreeMap::new();
        let Some(mask_prop) = node.property("interrupt-map-mask") else {
            kwarn!("pci: host bridge missing interrupt-map-mask; INTx will not work");
            return Self { entries };
        };
        let Some(map_prop) = node.property("interrupt-map") else {
            kwarn!("pci: host bridge missing interrupt-map; INTx will not work");
            return Self { entries };
        };
        let mask_bytes = mask_prop.value;
        let map_bytes = map_prop.value;
        if mask_bytes.len() < 16 {
            return Self { entries };
        }
        // Entry layout (child-spec 3 cells, child-intr 1 cell, parent
        // phandle 1 cell, parent-spec 2 cells) = 7 cells = 28 B.
        let dev_mask = u32::from_be_bytes(mask_bytes[0..4].try_into().unwrap());
        let pin_mask = u32::from_be_bytes(mask_bytes[12..16].try_into().unwrap());
        for chunk in map_bytes.chunks_exact(28) {
            let child = u32::from_be_bytes(chunk[0..4].try_into().unwrap());
            let pin = u32::from_be_bytes(chunk[12..16].try_into().unwrap());
            let parent_irq = u32::from_be_bytes(chunk[20..24].try_into().unwrap());
            entries.insert((child & dev_mask, pin & pin_mask), parent_irq);
        }
        Self { entries }
    }

    pub(super) fn lookup(&self, device: u8, pin: u8) -> Option<u32> {
        if !(1..=4).contains(&pin) {
            return None;
        }
        let device_key = (device as u32) << 11;
        self.entries.get(&(device_key, pin as u32)).copied()
    }
}
