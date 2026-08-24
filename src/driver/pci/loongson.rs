use fdt::node::FdtNode;

use crate::driver::Device;
use crate::{kinfo, kwarn};

use super::interrupt::MsiAllocator;

const PCH_MSI_COMPATIBLE: &str = "loongson,pch-msi-1.0";

pub(super) fn msi_allocator_from_fdt<'b, 'a: 'b>(
    device: &Device<'b, 'a>,
    host_node: &FdtNode<'b, 'a>,
) -> Option<MsiAllocator> {
    let controller = find_msi_controller(device, host_node)?;
    if !controller
        .compatible()
        .is_some_and(|compatible| compatible.all().any(|compatible| compatible == PCH_MSI_COMPATIBLE))
    {
        kwarn!("pci: unsupported MSI controller `{}`", controller.name);
        return None;
    }

    let (doorbell, _) = first_reg(&controller)?;
    let first_irq = read_u32(&controller, "loongson,msi-base-vec")?;
    let irq_count = read_u32(&controller, "loongson,msi-num-vecs")?;
    if irq_count == 0 {
        kwarn!("pci: MSI controller `{}` has no vectors", controller.name);
        return None;
    }
    let end_irq = first_irq.checked_add(irq_count)?;

    kinfo!(
        "pci: MSI doorbell {:#x}, vectors {}..{}",
        doorbell,
        first_irq,
        end_irq - 1,
    );

    Some(MsiAllocator::new(doorbell as u64, first_irq, end_irq))
}

fn find_msi_controller<'b, 'a: 'b>(device: &Device<'b, 'a>, host_node: &FdtNode<'b, 'a>) -> Option<FdtNode<'b, 'a>> {
    if let Some(prop) = host_node.property("msi-map") {
        for chunk in prop.value.chunks_exact(16) {
            let phandle = u32::from_be_bytes(chunk[4..8].try_into().ok()?);
            if let Some(node) = device.find_phandle(phandle) {
                return Some(node);
            }
        }
    }

    let prop = host_node.property("msi-parent")?;
    let phandle = u32::from_be_bytes(prop.value.get(0..4)?.try_into().ok()?);
    device.find_phandle(phandle)
}

fn first_reg(node: &FdtNode) -> Option<(usize, usize)> {
    let mut regions = node.reg()?;
    let region = regions.next()?;
    Some((region.starting_address as usize, region.size? as usize))
}

fn read_u32(node: &FdtNode, name: &str) -> Option<u32> {
    let value = node.property(name)?.value;
    Some(u32::from_be_bytes(value.get(0..4)?.try_into().ok()?))
}
