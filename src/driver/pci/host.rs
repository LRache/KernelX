use alloc::vec::Vec;

use fdt::node::FdtNode;
use virtio_drivers::transport::pci::bus::{BarInfo, Cam, Command, DeviceFunction, MemoryBarType, PciRoot};

use crate::driver::manager::found_device;
use crate::driver::{Device, PciInterrupt};
use crate::{arch, kinfo, kwarn};

use super::interrupt::{IntxTable, enable_msi, enable_msix};
use super::{config, loongson};

pub(super) fn scan_bus(device: &Device) {
    let Some((config_pa, config_size)) = device.mmio() else {
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

    let intx_table = IntxTable::from_fdt(&node);
    let mut msi = loongson::msi_allocator_from_fdt(device, &node);

    kinfo!(
        "pci: ECAM @ {:#x}..{:#x}, MMIO window {:#x}..{:#x}",
        config_pa,
        config_pa + config_size,
        mmio_pa,
        mmio_pa + mmio_size,
    );

    let cam = host_cam(&node);
    let config_kaddr = arch::mmio_phys_to_kaddr(config_pa, config_size);
    // SAFETY: `config_kaddr` maps the complete host configuration-space region
    // described by the matched PCI host bridge, using its advertised CAM type.
    let mut root = unsafe { PciRoot::new(config_kaddr as *mut u8, cam) };

    let mut allocator = BumpAllocator {
        next: align_up(mmio_pa as u64, 0x1000),
        end: (mmio_pa + mmio_size) as u64,
    };

    for (df, info) in root.enumerate_bus(0) {
        if info.class == 0x06 {
            continue;
        }

        let bars = match assign_bars(&mut root, df, &mut allocator) {
            Ok(bars) => bars,
            Err(error) => {
                kwarn!("pci: BAR assignment failed for {}: {:?}", df, error);
                continue;
            }
        };

        let base_command = Command::IO_SPACE | Command::MEMORY_SPACE | Command::BUS_MASTER;
        root.set_command(df, base_command);

        let pin = config::read_u8(config_kaddr, cam, df, 0x3d);
        let intx_irq = intx_table.lookup(df.device, pin);
        let message_interrupt = if let Some(msi) = msi.as_mut() {
            if let Some(interrupt) = enable_msix(config_kaddr, cam, &root, &bars, df, msi) {
                Some(interrupt)
            } else {
                enable_msi(config_kaddr, cam, &root, df, msi).map(PciInterrupt::Msi)
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

        let mut pci_device = Device::new_pci(&mut root, config_kaddr, cam, df, info, bars, interrupt);
        found_device(&mut pci_device);
    }
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
        let address = align_up(self.next, align);
        let next = address.checked_add(size)?;
        if next > self.end {
            return None;
        }
        self.next = next;
        Some(address)
    }
}

fn align_up(value: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

fn assign_bars(
    root: &mut PciRoot,
    df: DeviceFunction,
    allocator: &mut BumpAllocator,
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
                let address = allocator.alloc(size as u64, size as u64).ok_or(BarError::NoSpace)?;
                match address_type {
                    MemoryBarType::Width32 => {
                        root.set_bar_32(df, bar_index, address as u32);
                        bars[bar_index as usize] = Some(BarInfo::Memory {
                            address_type,
                            prefetchable,
                            address,
                            size,
                        });
                        bar_index += 1;
                    }
                    MemoryBarType::Width64 => {
                        root.set_bar_64(df, bar_index, address);
                        bars[bar_index as usize] = Some(BarInfo::Memory {
                            address_type,
                            prefetchable,
                            address,
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
    let bytes = node.property("ranges")?.value;
    let mut candidates = Vec::new();
    for chunk in bytes.chunks_exact(28) {
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
