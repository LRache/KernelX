mod abi;
mod device;
mod dtb;
mod fd;
mod guest_boot;
mod kvm;
mod terminal;
mod vcpu;

use std::sync::{Arc, Mutex};

use device::bus::{Bus, BusBuilder, DeviceRef};
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

async fn run() -> Result<(), String> {
    let options = parse_args()?;
    let mut bus_builder = BusBuilder::default();

    let uart: DeviceRef = Arc::new(Mutex::new(Uart16650Device::default()));
    bus_builder.add_mmio_device(UART0_BASE, Uart16650Device::LENGTH, uart, UART0_IRQ)?;

    let rtc: DeviceRef = Arc::new(Mutex::new(GoldfishRtcDevice::default()));
    bus_builder.add_mmio_device(RTC_BASE, GoldfishRtcDevice::LENGTH, rtc, RTC_IRQ)?;

    if let Some(disk_path) = options.disk_path.as_deref() {
        let virtio_blk: DeviceRef = Arc::new(Mutex::new(VirtioBlkDevice::open(disk_path).await?));
        bus_builder.add_mmio_device(VIRTIO_BLK_BASE, VirtioBlkDevice::LENGTH, virtio_blk, VIRTIO_BLK_IRQ)?;
    }

    let plic: DeviceRef = Arc::new(Mutex::new(PlicDevice::default()));
    bus_builder.add_mmio_device(PLIC_BASE, PlicDevice::LENGTH, plic, 0)?;

    let kvm = Kvm::open(bus_builder.build())?;

    let mut stdin_termios_guard = StdinTermiosGuard::default();
    stdin_termios_guard.enable_raw_input();
    stdin_termios_guard.enable_nonblocking_input();
    let _runtime_tasks = Bus::spawn_runtime_tasks(kvm.bus())?;

    let mapping = kvm.add_memory(options.memory_size)?;
    let entry = prepare_guest(&kvm, mapping, &options)?;
    boot_guest(&kvm, mapping, entry).await
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
