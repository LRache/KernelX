//! LoongArch FDT walker.
//!
//! Parses the flat device tree QEMU's `loongarch_direct_kernel_boot` path
//! leaves at a fixed PA (see `FDT_BASE_PA` below), then:
//!   1. Initializes the two interrupt controllers (EIOINTC + PCH-PIC) by
//!      matching their compatible strings — these MUST be up before we
//!      start matching per-device drivers, otherwise `enable_device_interrupt_irq`
//!      would fire on a dead bus.
//!   2. Walks the remaining top-level nodes and feeds each one to
//!      `driver::found_device`, which matches compatible against the
//!      registered driver matchers.
//!   3. Parses `/chosen/bootargs` (QEMU writes `-append` into here
//!      automatically) and calls `parse_boot_args` so the kinit thread
//!      can pick up `root=`, `init=`, etc.
//!
//! Why the PA is hard-coded instead of being passed through `main()`:
//!   - RISC-V stashes the OpenSBI-supplied FDT pointer in the C symbol
//!     `__riscv_copied_fdt` (see `clib/src/arch/riscv/entry/fdt.c`). The
//!     kernel's `main()` signature stays `(hartid, heap_start, memory_top)`
//!     — FDT discovery is entirely arch-internal.
//!   - On LoongArch the same convention holds: QEMU does not hand us a
//!     register pointing at the FDT (the legacy `$a1` slot carries a
//!     cmdline VA instead). Instead, QEMU's `hw/loongarch/virt.c` always
//!     loads the DTB at `FDT_BASE = 0x100000`. We pick it up there.
//!
//! Structural differences from `src/arch/riscv/fdt.rs`:
//!   - LoongArch QEMU virt flattens everything under `/` — there is no
//!     `/soc` bus node. We iterate `fdt.root().children()`.
//!   - No PLIC; instead a two-layer EIOINTC (IOCSR) + PCH-PIC (MMIO)
//!     topology is handled below.
//!   - CPU count / timebase come from CPUCFG, not FDT, so there's no
//!     `load_cpu_node` analog.

use fdt::Fdt;
use fdt::node::FdtNode;

use crate::arch;
use crate::driver::{Device, found_device};
use crate::kernel::parse_boot_args;
use crate::{kinfo, kwarn};

use super::{eiointc, pch_pic, pci};

/// Fixed PA of the DTB blob loaded by QEMU's `loongarch_direct_kernel_boot`
/// path (see `include/hw/loongarch/virt.h` `#define FDT_BASE 0x100000`).
/// QEMU copies the user-supplied `-dtb` file — or a machine-synthesized one
/// — to this address before jumping into the kernel. There is no
/// mechanism for passing the PA through a register, so we encode the
/// convention directly. This mirrors how RISC-V uses the static symbol
/// `__riscv_copied_fdt` from init.c: each arch keeps FDT discovery
/// internal rather than plumbing it through `main()` parameters.
const FDT_BASE_PA: usize = 0x100000;

// EIOINTC compatibles we care about. QEMU virt uses `ls2k2000-eiointc` but
// hardware variants exist; match them all.
const EIOINTC_COMPATIBLE: &[&str] = &[
    "loongson,ls2k2000-eiointc",
    "loongson,ls2k0500-eiointc",
    "loongson,eiointc",
];

// PCH-PIC compatibles.
const PCH_PIC_COMPATIBLE: &[&str] = &["loongson,pch-pic-1.0", "loongson,pch-pic"];

/// Top-level entry: locate the FDT at the fixed PA, bring up interrupt
/// controllers, enumerate devices, consume bootargs.
pub fn load_device_tree() -> Result<(), ()> {
    let fdt_pa = FDT_BASE_PA;
    // FDT blob lives in RAM — DMW1 (cached) is the right window to read.
    let fdt_kaddr = arch::paddr_to_kaddr(fdt_pa);

    // First peek at magic + totalsize from a 2-word slice; then re-borrow
    // the full slice. Matches the RISC-V pattern so the shape of a panic
    // ("bad FDT magic") is consistent across arches.
    let probe = unsafe { core::slice::from_raw_parts(fdt_kaddr as *const u32, 2) };
    let magic = u32::from_be(probe[0]);
    if magic != 0xd00dfeed {
        kwarn!(
            "loongarch: FDT magic mismatch at PA {:#x} (got {:#x}, want 0xd00dfeed)",
            fdt_pa, magic,
        );
        return Err(());
    }
    let total_size = u32::from_be(probe[1]) as usize;
    let data: &'static [u8] = unsafe { core::slice::from_raw_parts(fdt_kaddr as *const u8, total_size) };

    let fdt = Fdt::new(data).map_err(|e| {
        kwarn!("loongarch: FDT parse error: {:?}", e);
    })?;

    // 1. Bring up interrupt controllers first.
    init_interrupt_controllers(&fdt);

    // 2. Walk the root's children and register every device that isn't
    //    already handled as an interrupt controller.
    let root = fdt.find_node("/").ok_or_else(|| {
        kwarn!("loongarch: FDT has no root node?!");
    })?;
    for child in root.children() {
        handle_root_child(&child);
    }

    // 3. Consume the kernel cmdline. On LoongArch QEMU virt, `-append` does
    //    **not** go into /chosen/bootargs — QEMU places the C-string in
    //    RAM and hands the PA in `$a1` at kernel entry (the LoongArch
    //    BPI protocol). entry.S stashes that PA into `la_boot_cmdline_pa`
    //    so we can retrieve it here. A zero value means "no cmdline";
    //    an all-zero string has the same effect as fallback.
    //
    //    FDT `/chosen/bootargs` is still checked first as a nicety — if
    //    ever QEMU (or a future hand-crafted DTB) does provide it,
    //    respect it.
    let chosen = fdt.chosen();
    if let Some(prop) = chosen.bootargs().filter(|s| !s.is_empty()) {
        parse_boot_args(prop);
    } else if let Some(cmdline) = read_boot_cmdline() {
        kinfo!("loongarch: cmdline from QEMU $a1: {:?}", cmdline);
        parse_boot_args(cmdline);
    } else {
        kwarn!("loongarch: no bootargs from FDT or $a1 — falling back to config defaults");
        // Seed an empty BOOT_ARGS so `kinit`'s BOOT_ARGS.get(...)
        // short-circuits to config::DEFAULT_* rather than deref'ing an
        // uninitialized InitedCell.
        parse_boot_args("");
    }

    kinfo!("loongarch: FDT walk complete");
    Ok(())
}

/// Match EIOINTC + PCH-PIC, initialize them in that order.
///
/// EIOINTC has no FDT `reg` (its registers live in IOCSR space), so we
/// only need it to appear in the tree as a phandle target.
/// PCH-PIC `reg` yields the MMIO base we forward to `pch_pic::init`.
fn init_interrupt_controllers(fdt: &Fdt) {
    // EIOINTC first — PCH-PIC raises events to it, so EIOINTC must be
    // accepting before we touch PCH-PIC.
    if let Some(_eiointc_node) = find_by_compatible(fdt, EIOINTC_COMPATIBLE) {
        eiointc::init();
    } else {
        kwarn!("loongarch: no EIOINTC node in FDT — device interrupts will not fire");
    }

    if let Some(node) = find_by_compatible(fdt, PCH_PIC_COMPATIBLE) {
        if let Some(mut regs) = node.reg() {
            if let Some(region) = regs.next() {
                let base = region.starting_address as usize;
                let size = region.size.unwrap_or(0x400);
                pch_pic::init(base, size);
            } else {
                kwarn!("loongarch: PCH-PIC node has no `reg`");
            }
        } else {
            kwarn!("loongarch: PCH-PIC node has no `reg` property");
        }
    } else {
        kwarn!("loongarch: no PCH-PIC node in FDT — device interrupts will not route");
    }
}

/// Is the FDT node one of our interrupt controllers? Those are handled
/// via `init_interrupt_controllers`, not through the driver matcher.
fn is_interrupt_controller(node: &FdtNode) -> bool {
    if let Some(compat) = node.compatible() {
        for c in compat.all() {
            if EIOINTC_COMPATIBLE.contains(&c) || PCH_PIC_COMPATIBLE.contains(&c) {
                return true;
            }
            // CPU-core interrupt controller (loongson,cpu-interrupt-controller)
            // is a virtual node that needs no driver. Skip silently.
            if c == "loongson,cpu-interrupt-controller" {
                return true;
            }
        }
    }
    false
}

/// Nodes under the FDT root we don't want to hand to driver matchers:
/// bookkeeping (cpus, chosen, memory), and the interrupt controllers.
fn should_skip(node: &FdtNode) -> bool {
    matches!(node.name, "cpus" | "chosen" | "cpu-map")
        || node.name.starts_with("memory")
        || node.name.starts_with("memory@")
        || is_interrupt_controller(node)
}

const PCIE_HOST_COMPATIBLE: &[&str] = &["pci-host-ecam-generic", "pci-host-cam-generic"];

fn handle_root_child(node: &FdtNode) {
    if should_skip(node) {
        return;
    }
    // PCIe host bridge: we own the enumeration (BAR allocation, virtio
    // discovery) instead of leaving it to the generic driver matcher.
    if let Some(compat) = node.compatible() {
        for c in compat.all() {
            if PCIE_HOST_COMPATIBLE.contains(&c) {
                pci::scan_pcie_bus(node);
                return;
            }
        }
    }
    found_device(&Device::new(node));
}

/// Linear scan for a node whose compatible list intersects `candidates`.
/// `Fdt::find_compatible` does the same but only takes one pass; ours is
/// explicit so we can reuse the match outcome.
fn find_by_compatible<'a, 'b>(fdt: &'b Fdt<'a>, candidates: &[&str]) -> Option<FdtNode<'b, 'a>> {
    for node in fdt.all_nodes() {
        if let Some(compat) = node.compatible() {
            for c in compat.all() {
                if candidates.contains(&c) {
                    return Some(node);
                }
            }
        }
    }
    None
}

// `la_boot_cmdline_pa` is written by `clib/src/arch/loongarch/entry/entry.S`
// immediately after BSS is cleared. It holds the PA QEMU stored the
// `-append` string at (see `hw/loongarch/boot.c::init_cmdline`). A value
// of 0 is ambiguous (QEMU places the string at PA 0 when it's the first
// blob), so `read_boot_cmdline` always attempts to dereference and falls
// back only if the string at that PA is empty.
unsafe extern "C" {
    static la_boot_cmdline_pa: u64;
}

/// Read the NUL-terminated C-string QEMU placed at `la_boot_cmdline_pa`.
/// Returns the cmdline without the NUL, or `None` if empty / unreadable.
///
/// ## Safety & PA validity
///
/// - The PA comes from QEMU's ROM loader; bogus values are not expected.
/// - DMW1 (cached) is the right view: the cmdline lives in low-memory
///   RAM (sub-256 MiB), not MMIO, so cached reads are fine.
/// - We cap the length at `COMMAND_LINE_SIZE = 512` which matches
///   QEMU's `init_cmdline` copy size.
fn read_boot_cmdline() -> Option<&'static str> {
    let pa = unsafe { core::ptr::read_volatile(&raw const la_boot_cmdline_pa) };
    if pa > (1u64 << 48) {
        // Suspicious PA; bail rather than deref garbage.
        return None;
    }
    let kaddr = arch::paddr_to_kaddr(pa as usize);
    let base = kaddr as *const u8;
    let mut len = 0usize;
    while len < 512 {
        let b = unsafe { core::ptr::read_volatile(base.add(len)) };
        if b == 0 {
            break;
        }
        len += 1;
    }
    if len == 0 {
        return None;
    }
    let slice = unsafe { core::slice::from_raw_parts(base, len) };
    core::str::from_utf8(slice).ok()
}
