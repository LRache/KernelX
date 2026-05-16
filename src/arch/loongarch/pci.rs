use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use fdt::node::FdtNode;
use spin::Mutex;
use virtio_drivers::transport::pci::bus::{BarInfo, Cam, Command, DeviceFunction, MemoryBarType, PciRoot};
use virtio_drivers::transport::pci::{PciTransport, virtio_device_type};
use virtio_drivers::transport::{DeviceType as VirtioDeviceType, Transport};

use crate::driver::block::VirtIOBlockDriver;
use crate::driver::virtio::VirtIOHal;
use crate::driver::{DriverOps, register_irq_handler};
use crate::{arch, kinfo, kwarn};

pub fn scan_pcie_bus(node: &FdtNode) {
    let (ecam_pa, ecam_size) = match first_reg(node) {
        Some(pair) => pair,
        None => {
            kwarn!("loongarch: PCIe bridge has no reg property");
            return;
        }
    };

    let (mmio_pa, mmio_size) = match parse_mmio_range(node) {
        Some(range) => range,
        None => {
            kwarn!("loongarch: PCIe bridge has no 32-bit MMIO range; BAR allocation impossible");
            return;
        }
    };

    let irq_table = IntxTable::from_fdt(node);

    kinfo!(
        "loongarch: PCIe ECAM @ {:#x}..{:#x}, MMIO window {:#x}..{:#x}",
        ecam_pa,
        ecam_pa + ecam_size,
        mmio_pa,
        mmio_pa + mmio_size,
    );

    let ecam_kaddr = arch::mmio_phys_to_kaddr(ecam_pa, ecam_size);
    let mut root = unsafe { PciRoot::new(ecam_kaddr as *mut u8, Cam::Ecam) };

    let mut alloc = BumpAllocator {
        next: align_up(mmio_pa as u64, 0x1000),
        end: (mmio_pa + mmio_size) as u64,
    };

    for (df, info) in root.enumerate_bus(0) {
        if info.class == 0x06 {
            continue;
        }

        if let Err(e) = assign_bars(&mut root, df, &mut alloc) {
            kwarn!(
                "loongarch: BAR assignment failed for {:02x}:{:02x}.{}: {:?}",
                df.bus,
                df.device,
                df.function,
                e,
            );
            continue;
        }

        root.set_command(df, Command::IO_SPACE | Command::MEMORY_SPACE | Command::BUS_MASTER);

        if let Some(vdev_type) = virtio_device_type(&info) {
            let pin = read_interrupt_pin(ecam_kaddr, df);
            let irq = irq_table.lookup(df.device, pin);

            match PciTransport::new::<VirtIOHal>(&mut root, df) {
                Ok(transport) => {
                    let name = device_name(df, vdev_type);
                    register_virtio_pci_device(name, transport, vdev_type, irq);
                }
                Err(e) => {
                    kwarn!(
                        "loongarch: virtio PciTransport::new failed for {:02x}:{:02x}.{}: {:?}",
                        df.bus,
                        df.device,
                        df.function,
                        e,
                    );
                }
            }
        }
    }
}

fn register_virtio_pci_device(
    name: alloc::string::String,
    transport: PciTransport,
    vdev_type: VirtioDeviceType,
    irq: Option<u32>,
) {
    match vdev_type {
        VirtioDeviceType::Block => {
            let driver: Arc<dyn DriverOps> = Arc::new(VirtIOBlockDriver::new(name.clone(), transport));
            kinfo!(
                "loongarch: PCIe block device registered as `{}` (INTx IRQ {:?})",
                name,
                irq,
            );
            crate::driver::register_matched_driver(driver.clone());
            if let Some(irq) = irq {
                register_irq_handler(irq, driver.clone());
                arch::enable_device_interrupt_irq(irq);
            } else {
                kwarn!(
                    "loongarch: `{}` has no INTx IRQ; device will not receive interrupts",
                    name
                );
            }
            crate::fs::devfs::add_device(name, driver);
        }
        VirtioDeviceType::Network => {
            kwarn!("loongarch: PCIe virtio-net ignored (networking deferred)");
        }
        other => {
            kwarn!("loongarch: unsupported virtio-pci device type: {:?}", other);
        }
    }
}

#[allow(dead_code)]
fn transport_device_type(t: &PciTransport) -> VirtioDeviceType {
    t.device_type()
}

use core::sync::atomic::{AtomicU32, Ordering};

static BLOCK_COUNTER: AtomicU32 = AtomicU32::new(0);

fn device_name(_df: DeviceFunction, vdev_type: VirtioDeviceType) -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    match vdev_type {
        VirtioDeviceType::Block => {
            let idx = BLOCK_COUNTER.fetch_add(1, Ordering::Relaxed);
            let _ = write!(&mut s, "virtio_block{}", idx);
        }
        VirtioDeviceType::Network => {
            let _ = write!(&mut s, "virtio_net");
        }
        other => {
            let _ = write!(&mut s, "virtio_{:?}", other);
        }
    }
    s
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

fn assign_bars(root: &mut PciRoot, df: DeviceFunction, alloc: &mut BumpAllocator) -> Result<(), BarError> {
    let bars = root.bars(df).map_err(|_| BarError::NoSpace)?;

    let mut bar_index = 0u8;
    while bar_index < 6 {
        let Some(info) = &bars[bar_index as usize] else {
            bar_index += 1;
            continue;
        };
        match *info {
            BarInfo::Memory { address_type, size, .. } if size > 0 => {
                let align = size as u64;
                let addr = alloc.alloc(size as u64, align).ok_or(BarError::NoSpace)?;
                match address_type {
                    MemoryBarType::Width32 => {
                        root.set_bar_32(df, bar_index, addr as u32);
                        bar_index += 1;
                    }
                    MemoryBarType::Width64 => {
                        root.set_bar_64(df, bar_index, addr);
                        bar_index += 2;
                    }
                    MemoryBarType::Below1MiB => {
                        // Legacy; not used by virtio-pci.
                        bar_index += 1;
                    }
                }
            }
            _ => {
                bar_index += 1;
            }
        }
    }
    Ok(())
}

fn first_reg(node: &FdtNode) -> Option<(usize, usize)> {
    let mut iter = node.reg()?;
    let region = iter.next()?;
    Some((region.starting_address as usize, region.size?))
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

fn read_interrupt_pin(ecam_kaddr: usize, df: DeviceFunction) -> u8 {
    let slot_off = ((df.bus as usize) << 20) | ((df.device as usize) << 15) | ((df.function as usize) << 12);
    let byte_ptr = (ecam_kaddr + slot_off + 0x3D) as *const u8;
    unsafe { arch::read_volatile(byte_ptr) }
}

struct IntxTable {
    entries: BTreeMap<(u32, u32), u32>,
}

impl IntxTable {
    fn from_fdt(node: &FdtNode) -> Self {
        let mut entries = BTreeMap::new();
        let Some(mask_prop) = node.property("interrupt-map-mask") else {
            kwarn!("loongarch: PCIe node missing interrupt-map-mask; INTx will not work");
            return Self { entries };
        };
        let Some(map_prop) = node.property("interrupt-map") else {
            kwarn!("loongarch: PCIe node missing interrupt-map; INTx will not work");
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

#[allow(dead_code)]
static LAST_TABLE: Mutex<Option<BTreeMap<(u32, u32), u32>>> = Mutex::new(None);
