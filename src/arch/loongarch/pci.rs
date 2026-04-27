//! ECAM-based PCIe host controller for LoongArch QEMU virt.
//!
//! QEMU's `-M virt` exposes a generic `pci-host-ecam-generic` bridge
//! (compatible = "pci-host-ecam-generic") with:
//!   - a CAM window at PA 0x2000_0000..0x2800_0000 (ECAM: 4 KiB per BDF × 256 MiB)
//!   - two memory BAR ranges: I/O @ 0x1800_4000 (ignored) and
//!     32-bit MMIO @ 0x4000_0000, 1 GiB
//!
//! QEMU does **not** auto-allocate device BARs on this bus — Linux takes
//! care of it at boot. We mimic that by walking the bus, sizing each BAR,
//! and carving allocations out of the `ranges` MMIO window.
//!
//! This module does not itself create drivers. `scan_pcie_bus` discovers
//! virtio-PCI functions and hands a ready-to-use `PciTransport` to
//! `driver::virtio::matcher::try_match_pci`.

use alloc::sync::Arc;
use alloc::vec::Vec;
use fdt::node::FdtNode;
use virtio_drivers::transport::pci::bus::{
    BarInfo, Cam, Command, DeviceFunction, MemoryBarType, PciRoot,
};
use virtio_drivers::transport::pci::virtio_device_type;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::{DeviceType as VirtioDeviceType, Transport};

use crate::arch;
use crate::driver::block::VirtIOBlockDriver;
use crate::driver::virtio::VirtIOHal;
use crate::{kinfo, kwarn};

/// Walks the PCIe bridge described by `node`, allocates BARs from the
/// FDT-advertised memory window, and reports each virtio function back
/// to the generic driver registry.
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

    kinfo!(
        "loongarch: PCIe ECAM @ {:#x}..{:#x}, MMIO window {:#x}..{:#x}",
        ecam_pa, ecam_pa + ecam_size,
        mmio_pa, mmio_pa + mmio_size,
    );

    // ECAM must be accessed uncached.
    let ecam_kaddr = arch::mmio_phys_to_kaddr(ecam_pa, ecam_size);
    let mut root = unsafe { PciRoot::new(ecam_kaddr as *mut u8, Cam::Ecam) };

    // Simple bump allocator carving BARs out of the MMIO window. We only
    // need this during boot; once matchers consumed the Transport, the
    // allocator is gone.
    let mut alloc = BumpAllocator {
        next: align_up(mmio_pa as u64, 0x1000),
        end: (mmio_pa + mmio_size) as u64,
    };

    // Walk bus 0. QEMU virt typically doesn't route subordinate buses, so
    // one bus covers everything we care about.
    for (df, info) in root.enumerate_bus(0) {
        // PCI-PCI bridges (class 0x06, subclass 0x04) have a completely
        // different BAR layout (only 2 BARs then bus/secondary regs).
        // Generic BAR iteration on them crashes `PciRoot::bars`. Skip
        // bridges entirely — we don't descend to secondary buses yet.
        if info.class == 0x06 {
            continue;
        }

        // Fresh devices have MASTER/MEMORY_SPACE disabled and no BAR
        // addresses. Assign BARs to every memory region before handing
        // off to any driver (virtio-drivers' PciTransport::new asserts
        // that BARs are already set up).
        if let Err(e) = assign_bars(&mut root, df, &mut alloc) {
            kwarn!(
                "loongarch: BAR assignment failed for {:02x}:{:02x}.{}: {:?}",
                df.bus, df.device, df.function, e,
            );
            continue;
        }

        // Enable I/O + memory space + bus master so the device responds.
        root.set_command(df, Command::IO_SPACE | Command::MEMORY_SPACE | Command::BUS_MASTER);

        // Virtio? Hand to the virtio matcher.
        if let Some(vdev_type) = virtio_device_type(&info) {
            match PciTransport::new::<VirtIOHal>(&mut root, df) {
                Ok(transport) => {
                    let name = device_name(df, vdev_type);
                    register_virtio_pci_device(name, transport, vdev_type);
                }
                Err(e) => {
                    kwarn!(
                        "loongarch: virtio PciTransport::new failed for {:02x}:{:02x}.{}: {:?}",
                        df.bus, df.device, df.function, e,
                    );
                }
            }
        }
    }
}

/// Shape the PciTransport into the appropriate generic driver and publish
/// it through the usual `DRIVERS` + `INTERRUPT_MAP` registries. Mirrors
/// what `virtio::Matcher::try_match` does for MMIO transports.
fn register_virtio_pci_device(
    name: alloc::string::String,
    transport: PciTransport,
    vdev_type: VirtioDeviceType,
) {
    match vdev_type {
        VirtioDeviceType::Block => {
            let driver = Arc::new(VirtIOBlockDriver::new(name.clone(), transport));
            kinfo!("loongarch: PCIe block device registered as `{}`", name);
            crate::driver::register_matched_driver(driver.clone());
            crate::fs::devfs::add_device(name, driver);
        }
        VirtioDeviceType::Network => {
            // Phase 6 target is block only; leave networking for Phase 7.
            kwarn!("loongarch: PCIe virtio-net ignored (Phase 7 will handle networking)");
        }
        other => {
            kwarn!("loongarch: unsupported virtio-pci device type: {:?}", other);
        }
    }
}

fn transport_device_type(t: &PciTransport) -> VirtioDeviceType {
    t.device_type()
}

use core::sync::atomic::{AtomicU32, Ordering};

/// Monotonic counter for device naming. Matches `config::DEFAULT_BOOT_ROOT
/// = "virtio_block0"` for the first block device. Starts at 0; every
/// call bumps it by one so we hand out `virtio_block0`, `virtio_block1`, ...
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
            // Networking stays disabled on LoongArch until Phase 7; name
            // is only used for the warn message, no uniqueness needed.
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

fn assign_bars(
    root: &mut PciRoot,
    df: DeviceFunction,
    alloc: &mut BumpAllocator,
) -> Result<(), BarError> {
    let bars = root
        .bars(df)
        .map_err(|_| BarError::NoSpace)?;

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

/// `reg = <...>` → (base, size) of the first tuple. Assumes #address-cells
/// and #size-cells of 2 on the parent (LoongArch QEMU virt matches).
fn first_reg(node: &FdtNode) -> Option<(usize, usize)> {
    let mut iter = node.reg()?;
    let region = iter.next()?;
    Some((region.starting_address as usize, region.size?))
}

/// Parse the PCIe `ranges` property and pick the first 32-bit / 64-bit
/// memory range (flag byte 0x02 or 0x03). Returns `(host_pa, size)`.
///
/// Layout per entry (2-cell addr / 2-cell size parent / 3-cell PCI addr):
///   [0..4]   PCI flags (u32, big-endian)
///   [4..12]  PCI address (u64 be)
///   [12..20] host PA (u64 be)
///   [20..28] size (u64 be)
fn parse_mmio_range(node: &FdtNode) -> Option<(usize, usize)> {
    let prop = node.property("ranges")?;
    let bytes = prop.value;
    let entry = 28; // bytes per range triple when cell sizes are (3, 2, 2)
    let mut candidates = Vec::new();
    for chunk in bytes.chunks_exact(entry) {
        let flags = u32::from_be_bytes(chunk[0..4].try_into().ok()?);
        let space = (flags >> 24) & 0x03;
        // space 0x02 = 32-bit mem, 0x03 = 64-bit mem.
        if space != 0x02 && space != 0x03 {
            continue;
        }
        let host_pa = u64::from_be_bytes(chunk[12..20].try_into().ok()?) as usize;
        let size = u64::from_be_bytes(chunk[20..28].try_into().ok()?) as usize;
        candidates.push((host_pa, size));
    }
    // Prefer a 32-bit-addressable window if present (first candidate).
    candidates.into_iter().next()
}
