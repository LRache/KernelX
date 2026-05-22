use fdt::Fdt;
use fdt::node::FdtNode;

use crate::arch::riscv::plic;
use crate::driver::{Device, found_device};
use crate::kernel::{config, parse_boot_args};
use crate::{kinfo, kwarn};

use super::cpu::load_cpu_node;

pub fn load_device_tree(fdt: *const u8) -> Result<(), ()> {
    let data = unsafe { core::slice::from_raw_parts(fdt as *const u32, 2) };
    let magic = u32::from_be(data[0]);
    if magic != 0xd00dfeed {
        return Err(());
    }

    let total_size = u32::from_be(data[1]) as usize;

    let data: &'static [u8] = unsafe { core::slice::from_raw_parts(fdt, total_size) };

    let fdt = Fdt::new(data).unwrap();

    let cpus_node = fdt.find_node("/cpus").unwrap();
    load_cpu_node(&cpus_node);

    let soc_node = fdt.find_node("/soc").unwrap();
    load_plic_node(&fdt, &soc_node);

    for child in soc_node.children() {
        load_soc_node(&fdt, child);
    }

    let chosen_node = fdt.find_node("/chosen").unwrap();
    match chosen_node.property("bootargs").and_then(|prop| prop.as_str()) {
        Some(bootargs) if !bootargs.trim().is_empty() => parse_boot_args(bootargs),
        Some(_) => {
            kinfo!("Empty bootargs found in /chosen node, using default bootargs");
            parse_boot_args(config::DEFAULT_BOOTARGS);
        }
        None => {
            kwarn!("No bootargs found in /chosen node");
            parse_boot_args(config::DEFAULT_BOOTARGS);
        }
    }

    kinfo!("Device Tree loaded successfully!");

    Ok(())
}

fn load_soc_node<'b, 'a: 'b>(fdt: &'b Fdt<'a>, child: FdtNode<'b, 'a>) {
    let mut device = Device::new(fdt, child);
    found_device(&mut device);
}

fn load_plic_node(fdt: &Fdt, soc_node: &FdtNode) {
    if let Some(child) = soc_node.children().find(|child| {
        child
            .compatible()
            .is_some_and(|compatibles| compatibles.all().into_iter().any(|c| c == "riscv,plic0"))
    }) {
        plic::from_fdt(fdt, &child);
    } else {
        plic::not_found();
    }
}

// fn load_cpu_node(child: &FdtNode) {
//     let isa_support = child.property("riscv,isa").and_then(|p| p.as_str()).unwrap_or("");
//     let extensions: Vec<&str> = isa_support.split('_').collect();
//     kinfo!("CPU ISA extensions: {:?}", extensions);
//     if extensions.iter().find(|&&ext| ext == "svadu").is_some() {
//         SVADU_EXTENSION_ENABLED.init(true);
//         kinfo!("SVADU extension is enabled");
//     } else {
//         SVADU_EXTENSION_ENABLED.init(false);
//         kinfo!("SVADU extension is disabled");
//     };
// }
