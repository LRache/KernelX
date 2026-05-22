use fdt::Fdt;
use fdt::node::FdtNode;

use crate::driver::{Device, found_device};
use crate::kernel::parse_boot_args;
use crate::{arch, kinfo, kwarn};

use super::{csr, eiointc, pch_pic};

const FDT_BASE_PA: usize = 0x100000;

const EIOINTC_COMPATIBLE: &[&str] = &[
    "loongson,ls2k2000-eiointc",
    "loongson,ls2k0500-eiointc",
    "loongson,eiointc",
];

const PCH_PIC_COMPATIBLE: &[&str] = &["loongson,pch-pic-1.0", "loongson,pch-pic"];

pub fn load_device_tree() -> Result<(), ()> {
    let fdt_kaddr = arch::paddr_to_kaddr(FDT_BASE_PA);

    // Peek magic + totalsize, then re-borrow the full blob.
    let probe = unsafe { core::slice::from_raw_parts(fdt_kaddr as *const u32, 2) };
    let magic = u32::from_be(probe[0]);
    if magic != 0xd00dfeed {
        kwarn!(
            "loongarch: FDT magic mismatch at PA {:#x} (got {:#x}, want 0xd00dfeed)",
            FDT_BASE_PA,
            magic,
        );
        return Err(());
    }
    let total_size = u32::from_be(probe[1]) as usize;
    let data: &'static [u8] = unsafe { core::slice::from_raw_parts(fdt_kaddr as *const u8, total_size) };

    let fdt = Fdt::new(data).map_err(|e| {
        kwarn!("loongarch: FDT parse error: {:?}", e);
    })?;

    init_interrupt_controllers(&fdt);

    let root = fdt.find_node("/").ok_or_else(|| {
        kwarn!("loongarch: FDT has no root node?!");
    })?;
    for child in root.children() {
        handle_root_child(&fdt, child);
    }

    // Cmdline: prefer FDT `/chosen/bootargs` if QEMU ever starts populating
    // it; otherwise fall back to `$a1` (LoongArch BPI).
    let chosen = fdt.chosen();
    if let Some(prop) = chosen.bootargs().filter(|s| !s.is_empty()) {
        parse_boot_args(prop);
    } else if let Some(cmdline) = read_boot_cmdline() {
        kinfo!("loongarch: cmdline from QEMU $a1: {:?}", cmdline);
        parse_boot_args(cmdline);
    } else {
        kwarn!("loongarch: no bootargs from FDT or $a1 — falling back to config defaults");
        parse_boot_args("");
    }

    kinfo!("loongarch: FDT walk complete");
    Ok(())
}

fn init_interrupt_controllers(fdt: &Fdt) {
    if let Some(eiointc_node) = find_by_compatible(fdt, EIOINTC_COMPATIBLE) {
        eiointc::init(eiointc_parent_line(&eiointc_node));
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

fn eiointc_parent_line(node: &FdtNode) -> usize {
    let Some(parent_line) = first_interrupt_cell(node).map(|line| line as usize) else {
        return csr::ecfg::LINE_HWI0;
    };
    if (csr::ecfg::LINE_HWI0..csr::ecfg::LINE_HWI0 + csr::ecfg::HWI_COUNT).contains(&parent_line) {
        parent_line
    } else {
        kwarn!(
            "loongarch: invalid EIOINTC parent interrupt {}; falling back to HWI0",
            parent_line
        );
        csr::ecfg::LINE_HWI0
    }
}

fn first_interrupt_cell(node: &FdtNode) -> Option<u32> {
    let prop = node.property("interrupts")?;
    let bytes = prop.value.get(0..4)?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

fn is_interrupt_controller(node: &FdtNode) -> bool {
    if let Some(compat) = node.compatible() {
        for c in compat.all() {
            if EIOINTC_COMPATIBLE.contains(&c) || PCH_PIC_COMPATIBLE.contains(&c) {
                return true;
            }
            // Virtual per-CPU controller — no driver needed.
            if c == "loongson,cpu-interrupt-controller" {
                return true;
            }
        }
    }
    false
}

fn should_skip(node: &FdtNode) -> bool {
    matches!(node.name, "cpus" | "chosen" | "cpu-map")
        || node.name.starts_with("memory")
        || node.name.starts_with("memory@")
        || is_interrupt_controller(node)
}

fn handle_root_child<'b, 'a: 'b>(fdt: &'b Fdt<'a>, node: FdtNode<'b, 'a>) {
    if should_skip(&node) {
        return;
    }
    let mut device = Device::new(fdt, node);
    found_device(&mut device);
}

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

unsafe extern "C" {
    static la_boot_cmdline_pa: u64;
}

fn read_boot_cmdline() -> Option<&'static str> {
    let pa = unsafe { core::ptr::read_volatile(&raw const la_boot_cmdline_pa) };
    if pa > (1u64 << 48) {
        return None;
    }
    let base = arch::paddr_to_kaddr(pa as usize) as *const u8;
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
