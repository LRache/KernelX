use fdt::node::FdtNode;
use fdt::Fdt;
use alloc::vec::Vec;

use crate::arch::riscv::plic;
use crate::kernel::parse_boot_args;
use crate::driver::Device;
use crate::driver::found_device;
use crate::klib::initcell::InitedCell;
use crate::{kinfo, kwarn};

static TIME_FREQ: InitedCell<u32> = InitedCell::uninit();
static SVADU_EXTENSION_ENABLED: InitedCell<bool> = InitedCell::uninit();
static CORE_COUNT: InitedCell<usize> = InitedCell::uninit();

pub fn load_device_tree(fdt: *const u8) -> Result<(), ()> {
    let data = unsafe { core::slice::from_raw_parts(fdt as *const u32, 2) };
    let magic = u32::from_be(data[0]);
    if magic != 0xd00dfeed {
        return Err(());
    }
        
    let total_size = u32::from_be(data[1]) as usize;

    let data: &'static [u8] = unsafe { core::slice::from_raw_parts(fdt, total_size) };

    let fdt = Fdt::new(data).unwrap();
    
    let cpu_node = fdt.find_node("/cpus").unwrap();
    let timebase_freq_prop = cpu_node.property("timebase-frequency").ok_or(())?;
    timebase_freq_prop.as_usize().map(|freq| {
        TIME_FREQ.init(freq as u32);
    });

    // let cpu_node = fdt.find_node("/cpus").unwrap();
    // let core_count = cpu_node.children().count();
    let core_count = 1;
    CORE_COUNT.init(core_count);
    kinfo!("Detected {} CPU cores", core_count);

    kinfo!("Init timebase frequency = {}Hz", *TIME_FREQ);
    
    let soc_node = fdt.find_node("/soc").unwrap();
    if soc_node.children().find_map(|child| {
        if child.compatible()?.all().into_iter().find(|compatible| *compatible == "riscv,plic0").is_some() {
            let mut reg_prop = child.reg()?;
            let reg = reg_prop.next()?;
            let addr = reg.starting_address as usize;
            let size = reg.size? as usize;
            plic::init(addr, size);
            Some(())
        } else {
            None
        }
    }).is_none() {
        plic::not_found();
    }
    
    for child in soc_node.children() {
        load_soc_node(&child);
    }

    load_cpu_node(&cpu_node.children().next().unwrap());

    let chosen_node = fdt.find_node("/chosen").unwrap();
    if let Some(bootargs_prop) = chosen_node.property("bootargs") {
        bootargs_prop.as_str().map(|bootargs| {
            parse_boot_args(bootargs);
        });
    } else {
        kwarn!("No bootargs found in /chosen node");
    }

    kinfo!("Device Tree loaded successfully!");

    Ok(())
}

fn load_soc_node(child: &FdtNode) -> Option<()> {
    let reg_prop = child.reg();
    if let Some(mut reg) = reg_prop {
        let reg = reg.next()?;
        let addr = reg.starting_address as usize;
        let size = reg.size? as usize;

        let compatible = child.compatible()?.first();
        
        let interrupts = child.property("interrupts").and_then(|p| p.as_usize()).map(|v| v as u32);

        let device = Device::new(
            addr, 
            size, 
            child.name, 
            compatible,
            interrupts
        );
        
        found_device(&device);
    }
    Some(())
}

fn load_cpu_node(child: &FdtNode) {
    let isa_support = child.property("riscv,isa").and_then(|p| p.as_str()).unwrap_or("");
    let extensions: Vec<&str> = isa_support.split('_').collect();
    if extensions.iter().find(|&&ext| ext == "svadu").is_some() {
        SVADU_EXTENSION_ENABLED.init(true);
        kinfo!("SVADU extension is enabled");
    } else {
        SVADU_EXTENSION_ENABLED.init(false);
        kinfo!("SVADU extension is disabled");
    };
}

pub fn time_frequency() -> u32 {
    *TIME_FREQ
}

pub fn svadu_enable() -> bool {
    *SVADU_EXTENSION_ENABLED
}

pub fn core_count() -> usize {
    *CORE_COUNT
}
