use alloc::vec::Vec;
use fdt::node::FdtNode;

use crate::kinfo;
use crate::klib::InitedCell;

pub struct CPUInfo {
    svadu_enabled: bool,
    #[allow(dead_code)]
    zbc_supported: bool,
    float_supported: bool,
    double_supported: bool,
}

impl CPUInfo {
    pub fn svadu_enabled(&self) -> bool {
        self.svadu_enabled
    }

    #[allow(dead_code)]
    pub fn zbc_supported(&self) -> bool {
        self.zbc_supported
    }

    pub fn float_supported(&self) -> bool {
        self.float_supported
    }

    pub fn double_supported(&self) -> bool {
        self.double_supported
    }
}

static CPU_INFO: InitedCell<Vec<(usize, CPUInfo)>> = InitedCell::uninit();
static TIME_FREQ: InitedCell<u32> = InitedCell::uninit();

pub fn load_cpu_node(cpus_node: &FdtNode) {
    let timebase_freq_prop = cpus_node.property("timebase-frequency").unwrap();
    if let Some(freq) = timebase_freq_prop.as_usize() {
        TIME_FREQ.init(freq as u32);
    }
    kinfo!("Init timebase frequency = {}Hz", *TIME_FREQ);

    let mut cpus = Vec::new();
    for child in cpus_node.children() {
        if child.property("device_type").and_then(|p| p.as_str()) != Some("cpu") {
            continue;
        }

        let Some(hart_id) = child.property("reg").and_then(|p| p.as_usize()) else {
            continue;
        };

        let isa_support = child.property("riscv,isa").and_then(|p| p.as_str()).unwrap_or("");
        let extensions: Vec<&str> = isa_support.split('_').collect();

        let svadu_enabled = has_extension(child, &extensions, "svadu");
        let zbc_supported = has_extension(child, &extensions, "zbc");

        let base = extensions.first().copied().unwrap_or("");
        let float_supported = base.contains('f') || has_extension(child, &extensions, "f");
        let double_supported = base.contains('d') || has_extension(child, &extensions, "d");

        cpus.push((
            hart_id,
            CPUInfo {
                svadu_enabled,
                zbc_supported,
                float_supported,
                double_supported,
            },
        ));
    }
    CPU_INFO.init(cpus);

    kinfo!("Detected {} CPU cores", CPU_INFO.len());
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
    CPU_INFO.len()
}

pub fn get_cpu_info(hart_id: usize) -> &'static CPUInfo {
    CPU_INFO
        .iter()
        .find(|(id, _)| *id == hart_id)
        .map(|(_, info)| info)
        .unwrap_or_else(|| panic!("unknown hart ID {}", hart_id))
}
