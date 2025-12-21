// Reference: https://github.com/mit-pdos/xv6-riscv/blob/riscv/kernel/uart.c

use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::driver::char::serial::SerialOps;
use crate::driver::char::serial::stty::Stty;
use crate::driver::{DriverMatcher, DriverOps, Device};
use crate::kernel::mm::page;
use crate::kernel::mm::MapPerm;
use crate::arch;

mod regs {
    pub const RHR: usize = 0; // receive holding register (for input bytes)
    pub const THR: usize = 0; // transmit holding register (for output bytes)
    pub const IER: usize = 1; // interrupt enable register
    pub const IER_RX_ENABLE: u8 = 1 << 0;
    pub const IER_TX_ENABLE: u8 = 1 << 1;
    pub const FCR: usize = 2; // FIFO control register
    pub const FCR_FIFO_ENABLE: u8 = 1 << 0;
    pub const FCR_FIFO_CLEAR: u8 = 3 << 1; // clear the content of the two FIFOs
    pub const ISR: usize = 2; // interrupt status register
    pub const LCR: usize = 3; // line control register
    pub const LCR_EIGHT_BITS: u8 = 3 << 0;
    pub const LCR_BAUD_LATCH: u8 = 1 << 7; // special mode to set baud rate
    pub const LSR: usize = 5; // line status register
    pub const LSR_RX_READY: u8 = 1 << 0; // input is waiting to be read from RHR
    pub const LSR_TX_IDLE: u8 = 1 << 5; // THR can accept another character to send
}

pub struct Serial {
    base: usize,
}

impl Serial {
    pub fn new(base: usize) -> Self {
        Self { base }
    }

    fn read_reg(&mut self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u8) }
    }

    fn write_reg(&mut self, offset: usize, value: u8) {
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u8, value); }
    }

    pub fn init(&mut self) {
        // Disable interrupts while configuring the UART.
        self.write_reg(regs::IER, 0x00);

        // Enter baud latch mode to configure baud rate.
        self.write_reg(regs::LCR, regs::LCR_BAUD_LATCH);

        // Configure baud rate divisor for 38.4K (LSB then MSB).
        self.write_reg(0, 0x03);
        self.write_reg(1, 0x00);
        // Leave baud latch mode and configure: 8 bits, no parity.
        self.write_reg(regs::LCR, regs::LCR_EIGHT_BITS);

        // Reset and enable FIFOs.
        self.write_reg(regs::FCR, regs::FCR_FIFO_ENABLE | regs::FCR_FIFO_CLEAR);

        // Enable receive interrupts.
        self.write_reg(regs::IER, regs::IER_RX_ENABLE);
    }
}

impl SerialOps for Serial {
    fn getchar(&mut self) -> Option<u8> {
        if (self.read_reg(regs::LSR) & regs::LSR_RX_READY) == 0 {
            None
        } else {
            Some(self.read_reg(regs::RHR))
        }
    }

    fn putchar(&mut self, c: u8) -> bool {
        if (self.read_reg(regs::LSR) & regs::LSR_TX_IDLE) == 0 {
            return false;
        }
        self.write_reg(regs::THR, c);
        true
    }
}

pub struct Matcher;

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        if device.compatible() == "ns16550a" {
            let size = device.mmio_size();
            let kbase = page::alloc_contiguous(arch::page_count(size));
            arch::map_kernel_addr(kbase, device.mmio_base(), size, MapPerm::RW);
            
            let mut serial = Serial::new(kbase);
            serial.init();
            let driver = Stty::new(device.name().into(), Box::new(serial));

            if let Some(irq) = device.interrupt_number() {
                arch::enable_device_interrupt_irq(irq);
            }
            
            Some(Arc::new(driver))
        } else {
            None
        }
    }
}
