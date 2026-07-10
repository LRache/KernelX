use alloc::vec::Vec;
use virtio_drivers::device::net::{TxBuffer, VirtIONet};
use virtio_drivers::transport::mmio::MmioTransport;

use crate::driver::NetDriverOps;
use crate::driver::virtio::VirtIOHal;
use crate::kernel::errno::{Errno, SysResult};
use crate::klib::SpinLock;
use crate::net::protocol::MacAddr;

const QUEUE_SIZE: usize = 16;
const BUF_LEN: usize = 2048;

pub struct VirtioNetDriver {
    driver: SpinLock<VirtIONet<VirtIOHal, MmioTransport, QUEUE_SIZE>>,
}

impl VirtioNetDriver {
    pub fn new(transport: MmioTransport) -> Self {
        let mut net = VirtIONet::new(transport, BUF_LEN).expect("failed to create VirtIONet");
        net.enable_interrupts();
        Self {
            driver: SpinLock::new(net, "VirtioNetDriver::driver"),
        }
    }

    /// Acknowledge the hardware interrupt. Called by Interface before draining packets.
    pub fn ack_interrupt(&self) {
        self.driver.lock().ack_interrupt();
    }
}

impl NetDriverOps for VirtioNetDriver {
    fn send_packet(&self, packet: &[u8]) -> SysResult<()> {
        let mut driver = self.driver.lock();
        let tx_buf = TxBuffer::from(packet);
        driver.send(tx_buf).map_err(|_| Errno::EIO)
    }

    fn recv_packets(&self) -> Vec<Vec<u8>> {
        let mut driver = self.driver.lock();
        driver.ack_interrupt();
        let mut packets = Vec::new();
        while driver.can_recv() {
            match driver.receive() {
                Ok(rx_buf) => {
                    packets.push(rx_buf.packet().to_vec());
                    let _ = driver.recycle_rx_buffer(rx_buf);
                }
                Err(_) => break,
            }
        }
        packets
    }

    fn mac_address(&self) -> MacAddr {
        MacAddr::from_octets(self.driver.lock().mac_address())
    }

    fn mtu(&self) -> usize {
        1500
    }
}
