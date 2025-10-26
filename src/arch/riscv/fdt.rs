use fdt::node::FdtNode;
use fdt::Fdt;

use crate::driver::Device;
use crate::driver::found_device;
use crate::kinfo;
use crate::klib::initcell::InitedCell;

static TIME_FREQ: InitedCell<u32> = InitedCell::new();

pub fn load_device_tree(fdt: *const u8) -> Result<(), ()> {
    let data = unsafe { core::slice::from_raw_parts(fdt as *const u32, 2) };
    let magic = u32::from_be(data[0]);
    if magic != 0xd00dfeed {
        return Err(());
    }
        
    let total_size = u32::from_be(data[1]) as usize;

    let data = unsafe { core::slice::from_raw_parts(fdt, total_size) };

    let fdt = Fdt::new(data).unwrap();
    
    let cpu_node = fdt.find_node("/cpus").unwrap();
    let timebase_freq_prop = cpu_node.property("timebase-frequency").ok_or(())?;
    timebase_freq_prop.as_usize().map(|freq| {
        TIME_FREQ.init(freq as u32);
    });

    kinfo!("Timebase frequency: {} Hz", TIME_FREQ.get());
    
    let soc_node = fdt.find_node("/soc").unwrap();
    for child in soc_node.children() {
        load_soc_node(&child);
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

        let compatible = child.compatible()?;

        let device = Device::new(addr, size, child.name, compatible.first());
        found_device(&device);
    }
    Some(())
}

pub fn time_frequency() -> u32 {
    *TIME_FREQ.get()
}
