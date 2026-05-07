mod abi;
mod device;
mod dtb;
mod fd;
mod guest_boot;
mod kvm;
mod terminal;
mod vcpu;

use std::cell::RefCell;
use std::rc::Rc;

use device::bus::{BusBuilder, DeviceRef};
use device::plic::PlicDevice;
use device::rtc::GoldfishRtcDevice;
use device::uart::Uart16650Device;
use device::virtio_blk::VirtioBlkDevice;
use guest_boot::{
    PLIC_BASE, RTC_BASE, RTC_IRQ, UART0_BASE, UART0_IRQ, VIRTIO_BLK_BASE, VIRTIO_BLK_IRQ, boot_guest, parse_args,
    prepare_guest,
};
use kvm::Kvm;
use terminal::StdinTermiosGuard;

fn run() -> Result<(), String> {
    let options = parse_args()?;
    let mut bus_builder = BusBuilder::default();

    let uart: DeviceRef = Rc::new(RefCell::new(Uart16650Device::default()));
    bus_builder.add_mmio_device(UART0_BASE, Uart16650Device::LENGTH, uart, UART0_IRQ)?;

    let rtc: DeviceRef = Rc::new(RefCell::new(GoldfishRtcDevice::default()));
    bus_builder.add_mmio_device(RTC_BASE, GoldfishRtcDevice::LENGTH, rtc, RTC_IRQ)?;

    if let Some(disk_path) = options.disk_path.as_deref() {
        let virtio_blk: DeviceRef = Rc::new(RefCell::new(VirtioBlkDevice::open(disk_path)?));
        bus_builder.add_mmio_device(VIRTIO_BLK_BASE, VirtioBlkDevice::LENGTH, virtio_blk, VIRTIO_BLK_IRQ)?;
    }

    let plic: DeviceRef = Rc::new(RefCell::new(PlicDevice::default()));
    bus_builder.add_mmio_device(PLIC_BASE, PlicDevice::LENGTH, plic, 0)?;

    let kvm = Kvm::open(bus_builder.build())?;

    let mut stdin_termios_guard = StdinTermiosGuard::default();
    stdin_termios_guard.enable_raw_input();

    let mapping = kvm.add_memory(options.memory_size)?;
    let entry = prepare_guest(&kvm, mapping, &options)?;
    boot_guest(&kvm, mapping, entry)
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
