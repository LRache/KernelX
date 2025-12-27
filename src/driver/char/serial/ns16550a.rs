// Reference: https://github.com/mit-pdos/xv6-riscv/blob/riscv/kernel/uart.c
// Reference: https://github.com/riscv-software-src/opensbi/blob/master/lib/utils/serial/uart8250.c

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
    pub const LSB: usize = 0; // baud rate divisor LSB (when LCR_BAUD_LATCH is set)
    pub const MSB: usize = 1; // baud rate divisor MSB (when
    pub const IER_RX_ENABLE: u32 = 1 << 0;
    pub const IER_TX_ENABLE: u32 = 1 << 1;
    pub const FCR: usize = 2; // FIFO control register
    pub const FCR_FIFO_ENABLE: u32 = 1 << 0;
    pub const FCR_FIFO_CLEAR: u32 = 3 << 1; // clear the content of the two FIFOs
    pub const ISR: usize = 2; // interrupt status register
    pub const LCR: usize = 3; // line control register
    pub const LCR_EIGHT_BITS: u32 = 3 << 0;
    pub const LCR_BAUD_LATCH: u32 = 1 << 7; // special mode to set baud rate
    pub const LSR: usize = 5; // line status register
    pub const LSR_RX_READY: u32 = 1 << 0; // input is waiting to be read from RHR
    pub const LSR_TX_IDLE: u32 = 1 << 5; // THR can accept another character to send
}

enum RegSize {
    U8,
    U16,
    U32,
}

struct Serial {
    base: usize,
    shift: usize,
    reg_size: RegSize,
}

impl Serial {
    pub fn new(base: usize, shift: usize, reg_size: RegSize) -> Self {
        Self { base, shift, reg_size }
    }

    fn read_reg(&mut self, offset: usize) -> u32 {
        match self.reg_size {
            RegSize::U8  => unsafe { arch::read_volatile((self.base + (offset << self.shift)) as *const u8 ) as u32 },
            RegSize::U16 => unsafe { arch::read_volatile((self.base + (offset << self.shift)) as *const u16) as u32 },
            RegSize::U32 => unsafe { arch::read_volatile((self.base + (offset << self.shift)) as *const u32) as u32 },
        }
    }

    fn write_reg(&mut self, offset: usize, value: u32) {
        match self.reg_size {
            RegSize::U8  => unsafe { arch::write_volatile((self.base + (offset << self.shift)) as *mut u8, value as u8); },
            RegSize::U16 => unsafe { arch::write_volatile((self.base + (offset << self.shift)) as *mut u16, value as u16); },
            RegSize::U32 => unsafe { arch::write_volatile((self.base + (offset << self.shift)) as *mut u32, value as u32); },
        }
    }

    pub fn init(&mut self, bdiv: u32) {
        // Disable interrupts while configuring the UART.
        self.write_reg(regs::IER, 0x00);

        // Enter baud latch mode to configure baud rate.
        self.write_reg(regs::LCR, regs::LCR_BAUD_LATCH);

        // Configure baud rate divisor (LSB then MSB).
        if bdiv != 0 {
            self.write_reg(regs::LSB, bdiv & 0xff);
            self.write_reg(regs::MSB, (bdiv >> 8) & 0xff);
        }
        
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
            Some(self.read_reg(regs::RHR) as u8)
        }
    }

    fn putchar(&mut self, c: u8) -> bool {
        if (self.read_reg(regs::LSR) & regs::LSR_TX_IDLE) == 0 {
            return false;
        }
        self.write_reg(regs::THR, c as u32);
        true
    }
}

pub struct Matcher;

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        static SUPPORTED_COMPAT: &[&str] = &["ns16550a", "snps,dw-apb-uart"];
        device.match_compatible(SUPPORTED_COMPAT)?;

        let (mmio_base, mmio_size) = device.mmio()?;
        let kbase = page::alloc_contiguous(arch::page_count(mmio_size));
        arch::map_kernel_addr(kbase, mmio_base, mmio_size, MapPerm::RW);

        let io_width = device.fdt_node().property("reg-io-width")
            .and_then(|p| p.as_usize())
            .unwrap_or(1);
        let reg_shift = device.fdt_node().property("reg-shift")
            .and_then(|p| p.as_usize())
            .unwrap_or(0);

        let reg_size = match io_width {
            1 => RegSize::U8,
            2 => RegSize::U16,
            4 => RegSize::U32,
            _ => return None,
        };

        let speed = device.fdt_node().property("current-speed")
            .and_then(|p| p.as_usize())
            .unwrap_or(38400);
        let freq = device.fdt_node().property("clock-frequency")
            .and_then(|p| p.as_usize())
            .unwrap_or(0);
        let bdiv = (freq + 8 * speed) / (16 * speed);
            
        let mut serial = Serial::new(kbase, reg_shift, reg_size);
        serial.init(bdiv as u32);
        let driver = Stty::new(device.name().into(), Box::new(serial));

        if let Some(irq) = device.interrupt_number() {
            arch::enable_device_interrupt_irq(irq);
        }
            
        Some(Arc::new(driver))
    }
}
