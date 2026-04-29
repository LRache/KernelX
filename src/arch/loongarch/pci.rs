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
//!
//! ## INTx routing
//!
//! Each PCIe function has a legacy INTx assertion (INTA..INTD). The PCIe
//! bridge DT node carries `interrupt-map-mask` + `interrupt-map` properties
//! that describe how `(bus:dev:fn, pin)` translates to a PCH-PIC IRQ number.
//! On QEMU virt the layout is fixed:
//!
//!   mask = <0x1800 0 0 0x07>   // dev[4:3] and pin[2:0]
//!   map  = for every dev in 0..4, for every pin in 1..5:
//!            parent-spec = <0x8003  (dev+pin-1) mod 4 + 0x10  4>
//!
//! i.e. classic PCI swizzle rooted at PCH-PIC IRQ 0x10. We parse the FDT
//! map into a 4×4 lookup table and, for every enumerated function, read
//! Interrupt Pin from config space offset 0x3D, index into the table, and
//! register the resulting IRQ with `driver::manager::INTERRUPT_MAP` +
//! `Arch::enable_device_interrupt_irq`.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use fdt::node::FdtNode;
use spin::Mutex;
use virtio_drivers::transport::pci::bus::{
    BarInfo, Cam, Command, DeviceFunction, MemoryBarType, PciRoot,
};
use virtio_drivers::transport::pci::virtio_device_type;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::{DeviceType as VirtioDeviceType, Transport};

use crate::arch;
use crate::driver::block::VirtIOBlockDriver;
use crate::driver::virtio::VirtIOHal;
use crate::driver::{DriverOps, register_irq_handler};
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

    let irq_table = IntxTable::from_fdt(node);

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
        // Also clear the INTx-disable bit so legacy interrupts propagate;
        // QEMU may set it after reset on some virtio-pci models.
        root.set_command(df, Command::IO_SPACE | Command::MEMORY_SPACE | Command::BUS_MASTER);

        // Virtio? Hand to the virtio matcher.
        if let Some(vdev_type) = virtio_device_type(&info) {
            // Resolve legacy INTx IRQ via the parsed interrupt-map. Must
            // be done BEFORE PciTransport::new consumes the root ref,
            // because read_interrupt_pin needs raw ECAM access.
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
    irq: Option<u32>,
) {
    match vdev_type {
        VirtioDeviceType::Block => {
            let driver: Arc<dyn DriverOps> =
                Arc::new(VirtIOBlockDriver::new(name.clone(), transport));
            kinfo!(
                "loongarch: PCIe block device registered as `{}` (INTx IRQ {:?})",
                name, irq,
            );
            crate::driver::register_matched_driver(driver.clone());
            if let Some(irq) = irq {
                register_irq_handler(irq, driver.clone());
                arch::enable_device_interrupt_irq(irq);
            } else {
                kwarn!("loongarch: `{}` has no INTx IRQ; device will not receive interrupts", name);
            }
            crate::fs::devfs::add_device(name, driver);
        }
        VirtioDeviceType::Network => {
            // Phase 6 target is block only; leave networking for later.
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
            // Networking stays disabled on LoongArch until we ship a
            // virtio-net driver; name is only used for the warn message,
            // no uniqueness needed.
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

/// Read PCI config byte 0x3D (Interrupt Pin) directly from ECAM. Returns
/// the raw 1..4 INTx pin, or None if the device reports "no interrupt".
///
/// We bypass `PciRoot::config_read_word` because it's `pub(crate)` in the
/// virtio-drivers crate. ECAM is simple enough that direct pointer math
/// is cheaper than pulling in another abstraction:
///
///   offset = (bus << 20) | (dev << 15) | (func << 12) | register
///
/// For 4 KiB CAM slots this matches what QEMU exposes at 0x2000_0000.
fn read_interrupt_pin(ecam_kaddr: usize, df: DeviceFunction) -> u8 {
    let slot_off = ((df.bus as usize) << 20)
        | ((df.device as usize) << 15)
        | ((df.function as usize) << 12);
    // Register 0x3C holds Interrupt Line (low byte) and Interrupt Pin (second byte).
    let byte_ptr = (ecam_kaddr + slot_off + 0x3D) as *const u8;
    unsafe { arch::read_volatile(byte_ptr) }
}

/// Parsed `interrupt-map` for the PCIe root complex — mapping
/// `(device, pin)` → PCH-PIC IRQ. pin is 1-based (INTA = 1).
///
/// On QEMU LoongArch virt the map is a classic PCI swizzle:
///   irq = 0x10 + (device + pin - 1) mod 4
///
/// but we parse the FDT-provided table rather than hard-coding the
/// formula, so that any future QEMU re-routing survives the port.
struct IntxTable {
    /// Map key: `(device_mask, pin)`. `device_mask` is the raw 3-byte PCI
    /// child-spec with the interrupt-map mask applied, i.e.
    /// `0x0000`, `0x0800`, `0x1000`, `0x1800` for dev 0..3.
    /// Value: PCH-PIC IRQ number.
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
        // child-spec = 3 cells (12 B), child-intr = 1 cell (4 B) per QEMU,
        // parent-spec = 1 cell phandle + 2 cells intr specifier per
        // `#interrupt-cells = <2>` on PCH-PIC. Total: 7 cells = 28 B.
        let dev_mask = u32::from_be_bytes(mask_bytes[0..4].try_into().unwrap());
        let pin_mask = u32::from_be_bytes(mask_bytes[12..16].try_into().unwrap());
        const ENTRY_LEN: usize = 28;
        for chunk in map_bytes.chunks_exact(ENTRY_LEN) {
            // 3-cell child spec: we care only about the first cell (bus/dev/fn).
            let child0 = u32::from_be_bytes(chunk[0..4].try_into().unwrap());
            let pin = u32::from_be_bytes(chunk[12..16].try_into().unwrap());
            // 1-cell phandle at [16..20], 2-cell parent spec at [20..28].
            let parent_irq = u32::from_be_bytes(chunk[20..24].try_into().unwrap());
            let dev_key = child0 & dev_mask;
            let pin_key = pin & pin_mask;
            entries.insert((dev_key, pin_key), parent_irq);
        }
        Self { entries }
    }

    /// Look up the PCH-PIC IRQ for `device` / `pin`. `pin` is the INTx pin
    /// value from PCI config space byte 0x3D (1..4); 0 means the device
    /// doesn't use legacy interrupts.
    fn lookup(&self, device: u8, pin: u8) -> Option<u32> {
        if !(1..=4).contains(&pin) {
            return None;
        }
        // child0's dev field sits at bits 11:15; mirror how QEMU generates
        // `(dev << 11)` in the interrupt-map.
        let dev_key = (device as u32) << 11;
        let pin_key = pin as u32;
        self.entries.get(&(dev_key, pin_key)).copied()
    }
}

// Keep a live reference to the parsed table. Useful for debugging from a
// debugger; not strictly necessary since lookup happens only during
// scan_pcie_bus.
#[allow(dead_code)]
static LAST_TABLE: Mutex<Option<BTreeMap<(u32, u32), u32>>> = Mutex::new(None);
