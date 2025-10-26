use device_tree_parser::{DeviceTreeNode, DeviceTreeParser, DtbError};

use crate::arch::map_kernel_addr;
use crate::driver::manager::found_device;
use crate::kernel::mm::MapPerm;
use crate::kwarn;

use super::Device;

fn output_dtb_error(error: DtbError) -> () {
    kwarn!("Device Tree Blob error: {:?}", error);
}

pub fn load_device_tree(fdt: *const u8) -> Result<(), ()> {
    let data = unsafe { core::slice::from_raw_parts(fdt as *const u32, 2) };
    let magic = u32::from_be(data[0]);
    if magic != 0xd00dfeed {
        return Err(());
    }
        
    let total_size = u32::from_be(data[1]) as usize;

    let data = unsafe { core::slice::from_raw_parts(fdt, total_size) };

    let parser = DeviceTreeParser::new(data);
    let root = parser.parse_tree().map_err(|_| ())?;

    load_fdt_node(&root, None);

    Ok(())
}

fn load_fdt_device(node: &DeviceTreeNode, parent: Option<&DeviceTreeNode>) -> Result<(), ()> {
    let reg_addr = node.translate_reg_addresses(parent).map_err(output_dtb_error)?;
    if reg_addr.len() != 1 {
        // kwarn!("Device node {:?} has unsupported reg entries", node.name);
        return Ok(());
    }

    let (addr, size) = reg_addr[0];

    let compatible = node.prop_string("compatible").ok_or(())?;

    let device = Device::new(
        addr as usize,
        size as usize,
        node.name,
        &compatible,
    );

    found_device(&device);

    Ok(())
}
