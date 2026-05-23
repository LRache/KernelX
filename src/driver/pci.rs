use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::mem::size_of;
use core::ptr::addr_of_mut;
use fdt::node::FdtNode;
use virtio_drivers::transport::pci::bus::{BarInfo, Cam, Command, DeviceFunction, MemoryBarType, PciRoot};

use crate::{arch, kinfo, kwarn};

use super::manager::found_device;
use super::{Device, DeviceType, DriverOps, MMIOMatcher as MMIOMatcherTrait, PciInterrupt};

const PCI_HOST_COMPATIBLE: &[&str] = &["pci-host-ecam-generic", "pci-host-cam-generic"];
const LOONGSON_PCH_MSI_COMPATIBLE: &str = "loongson,pch-msi-1.0";

pub struct MMIOMatcher;

impl MMIOMatcherTrait for MMIOMatcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        device.match_compatible(PCI_HOST_COMPATIBLE)?;

        scan_pcie_bus(device);

        Some(Arc::new(HostDriver {
            device_name: device.name().into(),
        }))
    }
}

struct HostDriver {
    device_name: String,
}

impl DriverOps for HostDriver {
    fn name(&self) -> &str {
        "pci_host_driver"
    }

    fn device_name(&self) -> String {
        self.device_name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Pci
    }
}

fn scan_pcie_bus(device: &Device) {
    let Some((ecam_pa, ecam_size)) = device.mmio() else {
        kwarn!("pci: host bridge has no reg property");
        return;
    };

    let node = device.fdt_node();
    let (mmio_pa, mmio_size) = match parse_mmio_range(&node) {
        Some(range) => range,
        None => {
            kwarn!("pci: host bridge has no 32-bit MMIO range; BAR allocation impossible");
            return;
        }
    };

    let irq_table = IntxTable::from_fdt(&node);
    let mut msi = MsiAllocator::from_fdt(device, &node);

    kinfo!(
        "pci: ECAM @ {:#x}..{:#x}, MMIO window {:#x}..{:#x}",
        ecam_pa,
        ecam_pa + ecam_size,
        mmio_pa,
        mmio_pa + mmio_size,
    );

    let cam = host_cam(&node);
    let ecam_kaddr = arch::mmio_phys_to_kaddr(ecam_pa, ecam_size);
    let mut root = unsafe { PciRoot::new(ecam_kaddr as *mut u8, cam) };

    let mut alloc = BumpAllocator {
        next: align_up(mmio_pa as u64, 0x1000),
        end: (mmio_pa + mmio_size) as u64,
    };

    for (df, info) in root.enumerate_bus(0) {
        if info.class == 0x06 {
            continue;
        }

        let bars = match assign_bars(&mut root, df, &mut alloc) {
            Ok(bars) => bars,
            Err(e) => {
                kwarn!("pci: BAR assignment failed for {}: {:?}", df, e);
                continue;
            }
        };

        let base_command = Command::IO_SPACE | Command::MEMORY_SPACE | Command::BUS_MASTER;
        root.set_command(df, base_command);

        let pin = read_interrupt_pin(ecam_kaddr, cam, df);
        let intx_irq = irq_table.lookup(df.device, pin);
        let message_interrupt = if let Some(msi) = msi.as_mut() {
            if let Some(interrupt) = enable_msix(ecam_kaddr, cam, &root, &bars, df, msi) {
                Some(interrupt)
            } else {
                enable_msi(ecam_kaddr, cam, &root, df, msi).map(PciInterrupt::Msi)
            }
        } else {
            None
        };
        let interrupt = message_interrupt.or_else(|| intx_irq.map(PciInterrupt::Intx));
        let mut command = base_command;
        if interrupt.is_some_and(PciInterrupt::is_message_signaled) {
            command |= Command::INTERRUPT_DISABLE;
        }
        root.set_command(df, command);

        let mut pci_device = Device::new_pci(&mut root, ecam_kaddr, cam, df, info, bars, interrupt);
        found_device(&mut pci_device);
    }
}

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

struct MsiAllocator {
    doorbell: u64,
    next_irq: u32,
    end_irq: u32,
}

impl MsiAllocator {
    fn from_fdt<'b, 'a: 'b>(device: &Device<'b, 'a>, host_node: &FdtNode<'b, 'a>) -> Option<Self> {
        let controller = find_msi_controller(device, host_node)?;
        if !controller.compatible().is_some_and(|compatible| {
            compatible
                .all()
                .any(|compatible| compatible == LOONGSON_PCH_MSI_COMPATIBLE)
        }) {
            kwarn!("pci: unsupported MSI controller `{}`", controller.name);
            return None;
        }

        let (doorbell, _) = first_reg(&controller)?;
        let first_irq = read_fdt_u32(&controller, "loongson,msi-base-vec")?;
        let irq_count = read_fdt_u32(&controller, "loongson,msi-num-vecs")?;
        if irq_count == 0 {
            kwarn!("pci: MSI controller `{}` has no vectors", controller.name);
            return None;
        }
        let end_irq = first_irq.checked_add(irq_count)?;

        kinfo!(
            "pci: MSI doorbell {:#x}, vectors {}..{}",
            doorbell,
            first_irq,
            end_irq - 1,
        );

        Some(Self {
            doorbell: doorbell as u64,
            next_irq: first_irq,
            end_irq,
        })
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

fn enable_msix(
    ecam_kaddr: usize,
    cam: Cam,
    root: &PciRoot,
    bars: &[Option<BarInfo>; 6],
    df: DeviceFunction,
    msi: &mut MsiAllocator,
) -> Option<PciInterrupt> {
    let cap = root
        .capabilities(df)
        .find(|cap| cap.id == PciCapabilityId::Msix as u8)?;
    let control = config_read_u16(ecam_kaddr, cam, df, cap.offset as u16 + 2);
    let table_entries = (control & 0x07ff) as usize + 1;
    let table_info = config_read_u32(ecam_kaddr, cam, df, cap.offset as u16 + 4);
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
    config_write_u16(ecam_kaddr, cam, df, cap.offset as u16 + 2, masked_control);
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
    config_write_u16(ecam_kaddr, cam, df, cap.offset as u16 + 2, enabled_control);

    kinfo!("pci: {} uses MSI-X IRQ {} vector {}", df, message.irq, vector);
    Some(PciInterrupt::Msix {
        irq: message.irq,
        vector,
    })
}

fn enable_msi(ecam_kaddr: usize, cam: Cam, root: &PciRoot, df: DeviceFunction, msi: &mut MsiAllocator) -> Option<u32> {
    let cap = root.capabilities(df).find(|cap| cap.id == PciCapabilityId::Msi as u8)?;
    let message = msi.alloc()?;
    let control = config_read_u16(ecam_kaddr, cam, df, cap.offset as u16 + 2);
    let is_64bit = control & msi_control::ADDRESS_64BIT != 0;
    if !is_64bit && message.address > u32::MAX as u64 {
        kwarn!(
            "pci: {} MSI doorbell {:#x} does not fit in 32-bit message address",
            df,
            message.address,
        );
        return None;
    }

    config_write_u32(ecam_kaddr, cam, df, cap.offset as u16 + 4, message.address as u32);
    let data_offset = if is_64bit {
        config_write_u32(
            ecam_kaddr,
            cam,
            df,
            cap.offset as u16 + 8,
            (message.address >> 32) as u32,
        );
        cap.offset as u16 + 12
    } else {
        cap.offset as u16 + 8
    };
    config_write_u16(ecam_kaddr, cam, df, data_offset, message.data);

    if control & msi_control::PER_VECTOR_MASKING != 0 {
        config_write_u32(ecam_kaddr, cam, df, data_offset + 4, 0);
    }

    let new_control = (control & !msi_control::MULTIPLE_MESSAGE_ENABLE_MASK) | msi_control::ENABLE;
    config_write_u16(ecam_kaddr, cam, df, cap.offset as u16 + 2, new_control);

    kinfo!("pci: {} uses MSI IRQ {}", df, message.irq);
    Some(message.irq)
}

fn find_msi_controller<'b, 'a: 'b>(device: &Device<'b, 'a>, host_node: &FdtNode<'b, 'a>) -> Option<FdtNode<'b, 'a>> {
    if let Some(prop) = host_node.property("msi-map") {
        for chunk in prop.value.chunks_exact(16) {
            let phandle = u32::from_be_bytes(chunk[4..8].try_into().ok()?);
            if let Some(node) = device.find_phandle(phandle) {
                return Some(node);
            }
        }
    }

    let prop = host_node.property("msi-parent")?;
    let phandle = u32::from_be_bytes(prop.value.get(0..4)?.try_into().ok()?);
    device.find_phandle(phandle)
}

#[derive(Debug)]
enum BarError {
    NoSpace,
}

struct BumpAllocator {
    next: u64,
    end: u64,
}

impl BumpAllocator {
    fn alloc(&mut self, size: u64, align: u64) -> Option<u64> {
        let addr = align_up(self.next, align);
        let new_next = addr.checked_add(size)?;
        if new_next > self.end {
            return None;
        }
        self.next = new_next;
        Some(addr)
    }
}

fn align_up(x: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (x + align - 1) & !(align - 1)
}

fn assign_bars(
    root: &mut PciRoot,
    df: DeviceFunction,
    alloc: &mut BumpAllocator,
) -> Result<[Option<BarInfo>; 6], BarError> {
    let mut bars = root.bars(df).map_err(|_| BarError::NoSpace)?;

    let mut bar_index = 0u8;
    while bar_index < 6 {
        let Some(info) = bars[bar_index as usize].clone() else {
            bar_index += 1;
            continue;
        };
        match info {
            BarInfo::Memory {
                address_type,
                prefetchable,
                size,
                ..
            } if size > 0 => {
                let align = size as u64;
                let addr = alloc.alloc(size as u64, align).ok_or(BarError::NoSpace)?;
                match address_type {
                    MemoryBarType::Width32 => {
                        root.set_bar_32(df, bar_index, addr as u32);
                        bars[bar_index as usize] = Some(BarInfo::Memory {
                            address_type,
                            prefetchable,
                            address: addr,
                            size,
                        });
                        bar_index += 1;
                    }
                    MemoryBarType::Width64 => {
                        root.set_bar_64(df, bar_index, addr);
                        bars[bar_index as usize] = Some(BarInfo::Memory {
                            address_type,
                            prefetchable,
                            address: addr,
                            size,
                        });
                        bar_index += 2;
                    }
                    MemoryBarType::Below1MiB => {
                        bar_index += 1;
                    }
                }
            }
            _ => {
                bar_index += 1;
            }
        }
    }
    Ok(bars)
}

fn host_cam(node: &FdtNode) -> Cam {
    if node
        .compatible()
        .is_some_and(|compatible| compatible.all().any(|c| c == "pci-host-cam-generic"))
    {
        Cam::MmioCam
    } else {
        Cam::Ecam
    }
}

fn parse_mmio_range(node: &FdtNode) -> Option<(usize, usize)> {
    let prop = node.property("ranges")?;
    let bytes = prop.value;
    let entry = 28;
    let mut candidates = Vec::new();
    for chunk in bytes.chunks_exact(entry) {
        let flags = u32::from_be_bytes(chunk[0..4].try_into().ok()?);
        let space = (flags >> 24) & 0x03;
        // 0x02 = 32-bit mem, 0x03 = 64-bit mem.
        if space != 0x02 && space != 0x03 {
            continue;
        }
        let host_pa = u64::from_be_bytes(chunk[12..20].try_into().ok()?) as usize;
        let size = u64::from_be_bytes(chunk[20..28].try_into().ok()?) as usize;
        candidates.push((host_pa, size));
    }
    candidates.into_iter().next()
}

fn first_reg(node: &FdtNode) -> Option<(usize, usize)> {
    let mut iter = node.reg()?;
    let region = iter.next()?;
    Some((region.starting_address as usize, region.size? as usize))
}

fn read_fdt_u32(node: &FdtNode, name: &str) -> Option<u32> {
    let prop = node.property(name)?;
    Some(u32::from_be_bytes(prop.value.get(0..4)?.try_into().ok()?))
}

fn read_interrupt_pin(ecam_kaddr: usize, cam: Cam, df: DeviceFunction) -> u8 {
    config_read_u8(ecam_kaddr, cam, df, 0x3D)
}

fn config_offset(cam: Cam, df: DeviceFunction, register_offset: u16) -> usize {
    let bdf = ((df.bus as usize) << 8) | ((df.device as usize) << 3) | df.function as usize;
    let config_off = match cam {
        Cam::MmioCam => bdf << 8,
        Cam::Ecam => bdf << 12,
    };
    config_off + register_offset as usize
}

fn config_read_u8(ecam_kaddr: usize, cam: Cam, df: DeviceFunction, register_offset: u16) -> u8 {
    let byte_ptr = (ecam_kaddr + config_offset(cam, df, register_offset)) as *const u8;
    unsafe { arch::read_volatile(byte_ptr) }
}

fn config_read_u16(ecam_kaddr: usize, cam: Cam, df: DeviceFunction, register_offset: u16) -> u16 {
    let ptr = (ecam_kaddr + config_offset(cam, df, register_offset)) as *const u16;
    unsafe { arch::read_volatile(ptr) }
}

fn config_read_u32(ecam_kaddr: usize, cam: Cam, df: DeviceFunction, register_offset: u16) -> u32 {
    let ptr = (ecam_kaddr + config_offset(cam, df, register_offset)) as *const u32;
    unsafe { arch::read_volatile(ptr) }
}

fn config_write_u16(ecam_kaddr: usize, cam: Cam, df: DeviceFunction, register_offset: u16, value: u16) {
    let ptr = (ecam_kaddr + config_offset(cam, df, register_offset)) as *mut u16;
    unsafe { arch::write_volatile(ptr, value) }
}

fn config_write_u32(ecam_kaddr: usize, cam: Cam, df: DeviceFunction, register_offset: u16, value: u32) {
    let ptr = (ecam_kaddr + config_offset(cam, df, register_offset)) as *mut u32;
    unsafe { arch::write_volatile(ptr, value) }
}

struct IntxTable {
    entries: BTreeMap<(u32, u32), u32>,
}

impl IntxTable {
    fn from_fdt(node: &FdtNode) -> Self {
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
        const ENTRY_LEN: usize = 28;
        for chunk in map_bytes.chunks_exact(ENTRY_LEN) {
            let child0 = u32::from_be_bytes(chunk[0..4].try_into().unwrap());
            let pin = u32::from_be_bytes(chunk[12..16].try_into().unwrap());
            let parent_irq = u32::from_be_bytes(chunk[20..24].try_into().unwrap());
            entries.insert((child0 & dev_mask, pin & pin_mask), parent_irq);
        }
        Self { entries }
    }

    fn lookup(&self, device: u8, pin: u8) -> Option<u32> {
        if !(1..=4).contains(&pin) {
            return None;
        }
        let dev_key = (device as u32) << 11;
        let pin_key = pin as u32;
        self.entries.get(&(dev_key, pin_key)).copied()
    }
}
