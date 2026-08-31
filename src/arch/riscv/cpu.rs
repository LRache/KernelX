use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};
use fdt::node::FdtNode;

use crate::klib::InitedCell;
use crate::{kinfo, kwarn};

pub struct ArchPerCpuData {
    svadu_enabled: bool,
    zbc_supported: bool,
    float_supported: bool,
    double_supported: bool,
    h_extension: bool,
    sstc_supported: bool,
}

impl ArchPerCpuData {
    pub const fn new() -> Self {
        Self {
            svadu_enabled: false,
            zbc_supported: false,
            float_supported: false,
            double_supported: false,
            h_extension: false,
            sstc_supported: false,
        }
    }

    pub fn svadu_enabled(&self) -> bool {
        self.svadu_enabled
    }

    pub fn zbc_supported(&self) -> bool {
        self.zbc_supported
    }

    pub fn float_supported(&self) -> bool {
        self.float_supported
    }

    pub fn double_supported(&self) -> bool {
        self.double_supported
    }

    pub fn h_extension(&self) -> bool {
        self.h_extension
    }

    pub fn sstc_supported(&self) -> bool {
        self.sstc_supported
    }

    pub fn update_from(&mut self, cpu_info: &CPUInfo) {
        self.svadu_enabled = cpu_info.svadu_enabled();
        self.zbc_supported = cpu_info.zbc_supported();
        self.float_supported = cpu_info.float_supported();
        self.double_supported = cpu_info.double_supported();
        self.h_extension = cpu_info.h_extension();
        self.sstc_supported = cpu_info.sstc_supported();
    }
}

pub struct CPUInfo {
    svadu_enabled: bool,
    zbc_supported: bool,
    float_supported: bool,
    double_supported: bool,
    h_extension: bool,
    sstc_supported: bool,
}

impl CPUInfo {
    pub fn svadu_enabled(&self) -> bool {
        self.svadu_enabled
    }

    pub fn zbc_supported(&self) -> bool {
        self.zbc_supported
    }

    pub fn float_supported(&self) -> bool {
        self.float_supported
    }

    pub fn double_supported(&self) -> bool {
        self.double_supported
    }

    pub fn h_extension(&self) -> bool {
        self.h_extension
    }

    pub fn sstc_supported(&self) -> bool {
        self.sstc_supported
    }

    const fn absent() -> Self {
        Self {
            svadu_enabled: false,
            zbc_supported: false,
            float_supported: false,
            double_supported: false,
            h_extension: false,
            sstc_supported: false,
        }
    }
}

/// Indexed by hart ID; `MAX_HART_COUNT` matches the kernel CPU mask width.
/// Entries are meaningful only for harts present in `HART_MASK`.
static CPU_INFO: CPUInfoArray = CPUInfoArray(UnsafeCell::new([const { CPUInfo::absent() }; MAX_HART_COUNT]));
static TIME_FREQ: InitedCell<u32> = InitedCell::uninit();

/// The maximum number of harts the kernel supports, bounded by the width of
/// the `usize` CPU mask used by SBI calls, the scheduler, and TLB flushing.
const MAX_HART_COUNT: usize = usize::BITS as usize;

/// A bitmask of every discovered hart, with bit `i` set for hart ID `i`.
static HART_MASK: AtomicUsize = AtomicUsize::new(0);

struct CPUInfoArray(UnsafeCell<[CPUInfo; MAX_HART_COUNT]>);

// SAFETY: The array is written once by `load_cpu_node` before secondary harts
// start and is read-only afterwards; readers synchronize through `HART_MASK`.
unsafe impl Sync for CPUInfoArray {}

pub fn load_cpu_node(cpus_node: &FdtNode) {
    let timebase_freq_prop = cpus_node.property("timebase-frequency").unwrap();
    if let Some(freq) = timebase_freq_prop.as_usize() {
        TIME_FREQ.init(freq as u32);
    }
    kinfo!("Init timebase frequency = {}Hz", *TIME_FREQ);

    let mut cpus = [const { CPUInfo::absent() }; MAX_HART_COUNT];
    let mut hart_mask = 0usize;
    for child in cpus_node.children() {
        if child.property("device_type").and_then(|p| p.as_str()) != Some("cpu") {
            continue;
        }

        // The `reg` property of a cpu node is the hart ID, which is the CPU
        // number used throughout the kernel (cpu_mask, SBI calls, PLIC).
        let Some(hart_id) = child.property("reg").and_then(|p| p.as_usize()) else {
            continue;
        };
        if hart_id >= MAX_HART_COUNT {
            kwarn!(
                "CPU node hart ID {} exceeds the kernel CPU mask width; ignoring it",
                hart_id
            );
            continue;
        }
        if hart_mask & (1usize << hart_id) != 0 {
            kwarn!(
                "Duplicate hart ID {} in the device tree; ignoring the later node",
                hart_id
            );
            continue;
        }

        let isa_support = child.property("riscv,isa").and_then(|p| p.as_str()).unwrap_or("");
        let extensions: Vec<&str> = isa_support.split('_').collect();

        let svadu_enabled = has_extension(child, &extensions, "svadu");
        let zbc_supported = has_extension(child, &extensions, "zbc");
        let sstc_supported = has_extension(child, &extensions, "sstc");

        let base = extensions.first().copied().unwrap_or("");
        let float_supported = base.contains('f') || has_extension(child, &extensions, "f");
        let double_supported = base.contains('d') || has_extension(child, &extensions, "d");
        // The H extension is single-letter, so it only appears in the base ISA
        // string, never in `riscv,isa-extensions`.
        let h_extension = base.contains('h');

        cpus[hart_id] = CPUInfo {
            svadu_enabled,
            zbc_supported,
            float_supported,
            double_supported,
            h_extension,
            sstc_supported,
        };
        hart_mask |= 1usize << hart_id;
    }
    // SAFETY: `load_cpu_node` runs once on the boot hart before secondary
    // harts exist, so no other CPU can access `CPU_INFO` concurrently.
    unsafe {
        *CPU_INFO.0.get() = cpus;
    }
    // The Release store publishes the array writes above.
    HART_MASK.store(hart_mask, Ordering::Release);

    kinfo!("Detected {} CPU cores", hart_mask.count_ones());
}

fn has_extension(cpu_node: FdtNode<'_, '_>, isa_extensions: &[&str], extension: &str) -> bool {
    isa_extensions.iter().any(|&ext| ext == extension)
        || cpu_node
            .property("riscv,isa-extensions")
            .is_some_and(|prop| prop.iter_str().any(|ext| ext == extension))
}

pub fn time_frequency() -> u32 {
    *TIME_FREQ
}

pub fn try_time_frequency() -> Option<u32> {
    TIME_FREQ.try_get().copied()
}

pub fn core_count() -> usize {
    HART_MASK.load(Ordering::Acquire).count_ones() as usize
}

/// A bitmask of every hart discovered in the device tree, with bit `i` set
/// for hart ID `i`. This is the encoding shared by `cpu_mask`, SBI
/// hart-mask arguments, and the scheduler idle bitmap.
pub fn hart_mask() -> usize {
    HART_MASK.load(Ordering::Acquire)
}

pub fn try_get_cpu_info(hart_id: usize) -> Option<&'static CPUInfo> {
    let hart_mask = HART_MASK.load(Ordering::Acquire);
    if hart_id >= MAX_HART_COUNT || hart_mask & (1usize << hart_id) == 0 {
        return None;
    }
    // SAFETY: The Acquire load of `HART_MASK` publishes the `CPU_INFO` entry
    // for every hart in the mask, and the array is never written afterwards.
    Some(unsafe { &(*CPU_INFO.0.get())[hart_id] })
}
