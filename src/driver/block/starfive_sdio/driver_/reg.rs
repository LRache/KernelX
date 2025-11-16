use core::fmt::Debug;

use bitflags::bitflags;

pub(crate) const REG_CTRL: usize = 0x000;
bitflags! {
    pub(crate) struct ControlMask: u32{
        const use_internal_dmac = 0b1 << 25;
        const enable_od_pullup = 0b1 << 24;
        const card_voltage_b = 0b1111 << 20;
        const card_voltage_a = 0b1111 << 16;
        const ceata_device_interrupt = 0b1 << 11;
        const send_auto_stop_ccsd = 0b1 << 10;
        const send_ccsd = 0b1 << 9;
        const abort_read_data = 0b1 << 8;
        const send_irq_response = 0b1 << 7;
        const read_wait = 0b1 << 6;
        const dma_enable = 0b1 << 5;
        const int_enable = 0b1 << 4;
        const dma_reset = 0b1 << 2;
        const fifo_reset = 0b1 << 1;
        const controller_reset = 0b1;
    }
}
pub(crate) const REG_PWREN: usize = 0x004;

pub(crate) const REG_CLKDIV: usize = 0x008;
pub(crate) const REG_CLKENA: usize = 0x010;

pub(crate) const REG_TMOUT: usize = 0x014;
pub(crate) const DATA_TMOUT_DEFUALT: u32 = 0xFFFFFF << 8;
pub(crate) const REG_CTYPE: usize = 0x018;
pub(crate) const REG_BLKSIZ: usize = 0x01C;
pub(crate) const BLKSIZ_DEFAULT: u32 = 0x200;

pub(crate) const REG_BYTCNT: usize = 0x020;

pub(crate) const REG_INTMASK: usize = 0x024;
pub(crate) const REG_RINTSTS: usize = 0x044;

bitflags! {
    #[derive(Debug)]
    pub(crate) struct InterruptMask: u32{
        const sdio_int_mask = 0xFFFF << 16;
        const ebe = 0b1 << 15;
        const acd = 0b1 << 14;
        const sbe = 0b1 << 13;
        const hle = 0b1 << 12;
        const frun = 0b1 << 11;
        const hto = 0b1 << 10;
        const drto = 0b1 << 9;
        const rto = 0b1 << 8;
        const dcrc = 0b1 << 7;
        const rcrc = 0b1 << 6;
        const rxdr = 0b1 << 5;
        const txdr = 0b1 << 4;
        const dto = 0b1 << 3;
        const cmd = 0b1 << 2;
        const re = 0b1 << 1;
        const cd = 0b1;
    }
}

pub(crate) const REG_CMDARG: usize = 0x028;
pub(crate) const REG_CMD: usize = 0x02C;
bitflags! {
    #[derive(Debug)]
    pub(crate) struct CmdMask:u32{
        const start_cmd = 0b1 << 31;
        const use_hold_reg = 0b1 << 29;
        const volt_switch = 0b1 << 28;
        const boot_mode = 0b1 << 27;
        const disable_boot = 0b1 << 26;
        const expect_boot_ack = 0b1 << 25;
        const enable_boot = 0b1 << 24;
        const ccs_expected = 0b1 << 23;
        const read_ceata_device = 0b1 << 22;
        const update_clock_registers_only = 0b1 << 21;
        const card_number = 0b11111 << 16;
        const send_initialization = 0b1 << 15;
        const stop_abort_cmd = 0b1 << 14;
        const wait_prvdata_complete = 0b1 << 13;
        const send_auto_stop = 0b1 << 12;
        const transfer_mode = 0b1 << 11;
        const write = 0b1 << 10;
        const data_expected = 0b1 << 9;
        const check_response_crc = 0b1 << 8;
        const response_length = 0b1 << 7;
        const response_expect = 0b1 << 6;
        const cmd_index = 0x3F;
    }
}

pub(crate) const REG_RESP0: usize = 0x030;
pub(crate) const REG_RESP1: usize = 0x034;
pub(crate) const REG_RESP2: usize = 0x038;
pub(crate) const REG_RESP3: usize = 0x03C;
pub(crate) const REG_STATUS: usize = 0x048;

bitflags! {
    #[derive(Debug)]
    pub(crate) struct StatusMask:u32{
        const dma_req= 0b1<< 31;
        const dma_ack= 0b1<< 30;
        const fifo_count= 0x1FFF<< 17;
        const response_index= 0x3F<< 11;
        const data_state_mc_busy= 0b1<< 10;
        const data_busy= 0b1<< 9;
        const data_3_status= 0b1<< 8;
        const command_fsm_states= 0xF<< 4;
        const fifo_full= 0b1<< 3;
        const fifo_empty= 0b1<< 2;
        const fifo_tx_watermark= 0b1<< 1;
        const fifo_rx_watermark= 0b1;
    }
}

// pub(crate) const REG_FIFOTH: u32 = 0x04C;
// pub(crate) const REG_CDETECT: u32 = 0x050;
// pub(crate) const REG_WRTPRT: u32 = 0x054;
// pub(crate) const REG_GPIO: u32 = 0x058;
// pub(crate) const REG_TCMCNT: u32 = 0x05C;
// pub(crate) const REG_TBBCNT: u32 = 0x060;
// pub(crate) const REG_DEBNCE: u32 = 0x064;
// pub(crate) const REG_USRID: u32 = 0x068;
// pub(crate) const REG_VERID: u32 = 0x06C;
pub(crate) const REG_HCON: usize = 0x070;

bitflags! {
    #[derive(Debug)]
    struct HardConfig: u32{
        const card_type = 0x1;
        const num_cards_sub1 = 0x1F<<1;
        const h_bus_type = 0x1 << 6;
        const h_data_width = 0b111<<7;
        const h_addr_width = 0x3F << 10;
        const dma_interface = 0b11 << 16;
        const ge_dma_data_width = 0b111 << 18;
        const fifo_ram_inside = 0b1 << 21;
        const impl_hold_reg = 0b1 << 22;
        const set_clk_false_path = 0b1 <<23;
        const num_clk_div_sub1 = 0b11<<24;
        const area_optimized = 0b1 << 26;
        const fifo_depth = 0x1F << 27;
    }
}

pub(crate) struct HardConf(u32);
impl From<u32> for HardConf {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl Debug for HardConf {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut f = f.debug_map();
        let conf = self.0;
        if conf | HardConfig::card_type.bits() != 0 {
            f.entry(&HardConfig::card_type, &"SD_MMC");
        } else {
            f.entry(&HardConfig::card_type, &"MMC_ONLY");
        }
        f.entry(
            &HardConfig::num_cards_sub1,
            &((conf & HardConfig::num_cards_sub1.bits()) >> 1),
        );
        if conf | HardConfig::h_bus_type.bits() != 0 {
            f.entry(&HardConfig::h_bus_type, &"AHB");
        } else {
            f.entry(&HardConfig::h_bus_type, &"APB");
        }
        let h_data = (conf & HardConfig::h_data_width.bits()) >> 7;
        match h_data {
            0 => {
                f.entry(&HardConfig::h_data_width, &"16 bit");
            }
            1 => {
                f.entry(&HardConfig::h_data_width, &"32 bit");
            }
            2 => {
                f.entry(&HardConfig::h_data_width, &"64 bit");
            }
            _ => {
                f.entry(&HardConfig::h_data_width, &"Other");
            }
        }
        f.entry(
            &HardConfig::h_addr_width,
            &((conf & HardConfig::h_addr_width.bits()) >> 10),
        );
        let dma_ifc = (conf & HardConfig::dma_interface.bits()) >> 16;
        match dma_ifc {
            0 => {
                f.entry(&HardConfig::dma_interface, &"None");
            }
            1 => {
                f.entry(&HardConfig::dma_interface, &"DW_DMA");
            }
            2 => {
                f.entry(&HardConfig::dma_interface, &"GENERIC_DMA");
            }
            3 => {
                f.entry(&HardConfig::dma_interface, &"NON-DW-DMA");
            }
            _ => {}
        }
        let ge_dma = (conf & HardConfig::ge_dma_data_width.bits()) >> 18;
        match ge_dma {
            0 => {
                f.entry(&HardConfig::ge_dma_data_width, &"None");
            }
            1 => {
                f.entry(&HardConfig::ge_dma_data_width, &"DW_DMA");
            }
            2 => {
                f.entry(&HardConfig::ge_dma_data_width, &"GENERIC_DMA");
            }
            _ => {
                f.entry(&HardConfig::ge_dma_data_width, &"Unknown");
            }
        }
        if conf | HardConfig::fifo_ram_inside.bits() != 0 {
            f.entry(&HardConfig::fifo_ram_inside, &"INSIDE");
        } else {
            f.entry(&HardConfig::fifo_ram_inside, &"OUTSIDE");
        }
        if conf | HardConfig::impl_hold_reg.bits() != 0 {
            f.entry(&HardConfig::impl_hold_reg, &"hold register");
        } else {
            f.entry(&HardConfig::impl_hold_reg, &"no hold register");
        }
        if conf | HardConfig::set_clk_false_path.bits() != 0 {
            f.entry(&HardConfig::set_clk_false_path, &"false path set");
        } else {
            f.entry(&HardConfig::set_clk_false_path, &"no false path");
        }
        f.entry(
            &HardConfig::num_clk_div_sub1,
            &((conf & HardConfig::num_clk_div_sub1.bits()) >> 24),
        );
        if conf | HardConfig::area_optimized.bits() != 0 {
            f.entry(&HardConfig::area_optimized, &"Area optimization");
        } else {
            f.entry(&HardConfig::area_optimized, &"no area optimization");
        }
        f.entry(
            &HardConfig::fifo_depth,
            &((conf & HardConfig::fifo_depth.bits()) >> 27),
        );
        f.finish()
    }
}

pub(crate) const REG_BMOD: usize = 0x080;

