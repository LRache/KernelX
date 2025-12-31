use alloc::vec::Vec;
use fdt::node::FdtNode;

use crate::klib::InitedCell;
use crate::kinfo;

pub struct CPUInfo {
    svadu_enabled: bool,
    float_supported: bool,
    double_supported: bool,
}

impl CPUInfo {
    pub fn svadu_enabled(&self) -> bool {
        self.svadu_enabled
    }

    pub fn float_supported(&self) -> bool {
        self.float_supported
    }

    pub fn double_supported(&self) -> bool {
        self.double_supported
    }
}

static CPU_INFO: InitedCell<Vec<CPUInfo>> = InitedCell::uninit();
static TIME_FREQ: InitedCell<u32> = InitedCell::uninit();

pub fn load_cpu_node(cpus_node: &FdtNode) {
    let timebase_freq_prop = cpus_node.property("timebase-frequency").unwrap();
    timebase_freq_prop.as_usize().map(|freq| {
        TIME_FREQ.init(freq as u32);
    });
    kinfo!("Init timebase frequency = {}Hz", *TIME_FREQ);
    
    let mut cpus = Vec::new();
    for child in cpus_node.children() {        
        let isa_support = child.property("riscv,isa").and_then(|p| p.as_str()).unwrap_or("");
        let extensions: Vec<&str> = isa_support.split('_').collect();
        
        let svadu_enabled = extensions.iter().find(|&&ext| ext == "svadu").is_some();

        let base = extensions[0];
        let float_supported =  base.contains('f'); 
        let double_supported = base.contains('d');

        cpus.push(CPUInfo {
            svadu_enabled,
            float_supported,
            double_supported,
        });
    }
    CPU_INFO.init(cpus);

    kinfo!("Detected {} CPU cores", CPU_INFO.len());
}

pub fn time_frequency() -> u32 {
    *TIME_FREQ
}

pub fn core_count() -> usize {
    CPU_INFO.len()
}

pub fn get_cpu_info(hart_id: usize) -> &'static CPUInfo {
    &CPU_INFO[hart_id]
}
