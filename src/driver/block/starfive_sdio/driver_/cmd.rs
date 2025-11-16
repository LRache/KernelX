use super::reg::CmdMask;
use core::fmt::Debug;

use super::sd_reg::{CardStatus, Cic, Cid, Csd, Ocr, Rca};

const ALL_SEND_CID: u32 = 2;
const SEND_RCA: u32 = 3;
const SWITCH_FUNCTION: u32 = 6;
const SELECT_CARD: u32 = 7;
const SEND_IF_COND: u32 = 8;
const SEND_CSD: u32 = 9;
const STOP_TRANSMISSION: u32 = 12;
const READ_SINGLE_BLOCK: u32 = 17;
const WRITE_SINGLE_BLOCK: u32 = 24;
const APP_CMD: u32 = 55;
const ACMD_SD_SEND_OP_COND: u32 = 41;
const ACMD_SET_BUS: u32 = 6;
#[derive(Clone, Copy, Default)]
pub struct Command {
    reg_flags: u32,
    index: u32,
    arg: u32,
    resp_ty: ResponseType,
}

impl Debug for Command {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Command ")
            .field("\n\tindex", &self.index)
            .field("\n\targ", &self.arg)
            .field("\n\tflags", &self.reg_flags)
            .field("\n\tresponse type", &self.resp_ty)
            .finish()
    }
}

impl Command {
    fn transfer_cmd(index: u32, resp_ty: ResponseType, addr: u32, write_mode: bool) -> Self {
        let mut cmd = Command::default();
        cmd.index = index;
        cmd.arg = addr;
        cmd.resp_ty = resp_ty;
        cmd.reg_flags |= CmdMask::start_cmd.bits()
            | CmdMask::use_hold_reg.bits()
            | CmdMask::data_expected.bits()
            | CmdMask::wait_prvdata_complete.bits()
            | CmdMask::response_expect.bits()
            | CmdMask::check_response_crc.bits();
        if write_mode {
            cmd.reg_flags |= CmdMask::write.bits();
        }
        cmd
    }

    fn no_data_cmd_r48(index: u32, resp_ty: ResponseType, arg: u32) -> Self {
        let mut cmd = Command::default();
        cmd.index = index;
        cmd.resp_ty = resp_ty;
        cmd.arg = arg;
        cmd.reg_flags |= CmdMask::start_cmd.bits()
            | CmdMask::use_hold_reg.bits()
            | CmdMask::wait_prvdata_complete.bits()
            | CmdMask::response_expect.bits()
            | CmdMask::check_response_crc.bits();
        cmd
    }
// low 5 bits are OP Num
    pub fn to_cmd(&self) -> u32 {
        self.reg_flags | self.index
    }

    pub fn arg(&self) -> u32 {
        self.arg
    }
    pub fn data_exp(&self) -> bool {
        self.reg_flags & CmdMask::data_expected.bits() != 0
    }

    pub fn resp_exp(&self) -> bool {
        self.resp_ty != ResponseType::Non
    }

    pub fn resp_lang(&self) -> bool {
        self.resp_ty == ResponseType::R2
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    Non = 0,
    R1 = 1,
    R1b = 10,
    R2 = 2,
    R3 = 3,
    R6 = 6,
    R7 = 7,
}

impl Default for ResponseType {
    fn default() -> Self {
        Self::Non
    }
}
impl Debug for ResponseType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Non => write!(f, "Non"),
            Self::R1 => write!(f, "R1"),
            Self::R1b => write!(f, "R1b"),
            Self::R2 => write!(f, "R2"),
            Self::R3 => write!(f, "R3"),
            Self::R6 => write!(f, "R6"),
            Self::R7 => write!(f, "R7"),
        }
    }
}

pub enum Response {
    Rz,
    R48(u32),
    R136((u32, u32, u32, u32)),
}

impl Response {
    pub(crate) fn card_status(self) -> CardStatus {
        match self {
            Response::R48(r) => CardStatus::from(r),
            _ => CardStatus::default(),
        }
    }
    pub(crate) fn csd(self) -> Csd {
        match self {
            Self::R136(r) => Csd::from(r),
            _ => Csd::default(),
        }
    }
    pub(crate) fn cid(self) -> Cid {
        match self {
            Self::R136(r) => Cid::from(r),
            _ => Cid::default(),
        }
    }

    pub(crate) fn ocr(self) -> Ocr {
        match self {
            Response::R48(r) => Ocr::from(r),
            _ => Ocr::default(),
        }
    }

    pub(crate) fn cic(self) -> Cic {
        match self {
            Response::R48(r) => Cic::from(r),
            _ => Cic::default(),
        }
    }

    pub(crate) fn rca(self) -> Rca {
        match self {
            Response::R48(r) => Rca::from(r),
            _ => Rca::default(),
        }
    }
}

pub fn idle() -> Command {
    let mut cmd = Command::default();
    cmd.reg_flags |= CmdMask::send_initialization.bits()
        | CmdMask::start_cmd.bits()
        | CmdMask::wait_prvdata_complete.bits()
        | CmdMask::use_hold_reg.bits();
    cmd
}

pub fn up_clk() -> Command {
    let mut cmd = Command::default();
    cmd.reg_flags |= CmdMask::update_clock_registers_only.bits()
        | CmdMask::wait_prvdata_complete.bits()
        | CmdMask::start_cmd.bits()
        | CmdMask::use_hold_reg.bits();
    cmd
}

/// CMD12: Stop transmission
pub fn stop_transmission() -> Command {
    let mut cmd = Command::default();
    cmd.reg_flags |= CmdMask::start_cmd.bits()
        | CmdMask::use_hold_reg.bits()
        | CmdMask::stop_abort_cmd.bits()
        | CmdMask::response_expect.bits()
        | CmdMask::check_response_crc.bits();
    cmd.index = STOP_TRANSMISSION;
    cmd.arg = 0;
    cmd.resp_ty = ResponseType::R1b;
    cmd
}

/// CMD2: Ask any card to send their CID
pub fn all_send_cid() -> Command {
    let mut cmd = Command::no_data_cmd_r48(ALL_SEND_CID, ResponseType::R2, 0);
    cmd.reg_flags |= CmdMask::response_length.bits();
    cmd
}

/// CMD6: switch function
pub fn switch_function(arg: u32) -> Command {
    Command::no_data_cmd_r48(SWITCH_FUNCTION, ResponseType::R1, arg)
}

/// CMD7: Select or deselect card
pub fn select_card(rca: u16) -> Command {
    let arg = u32::from(rca) << 16;
    Command::no_data_cmd_r48(SELECT_CARD, ResponseType::R1b, arg)
}

/// CMD9: Send CSD
pub fn send_csd(rca: u16) -> Command {
    let arg = u32::from(rca) << 16;
    let mut cmd = Command::no_data_cmd_r48(SEND_CSD, ResponseType::R2, arg);
    cmd.reg_flags |= CmdMask::response_length.bits();
    cmd
}

/// CMD17: Read a single block from the card
pub fn read_single_block(addr: u32) -> Command {
    Command::transfer_cmd(READ_SINGLE_BLOCK, ResponseType::R1, addr, false)
}

/// CMD24: Write block
pub fn write_single_block(addr: u32) -> Command {
    Command::transfer_cmd(WRITE_SINGLE_BLOCK, ResponseType::R1, addr, true)
}

/// CMD55: App Command. Indicates that next command will be a app command
pub fn app_cmd(rca: u16) -> Command {
    Command::no_data_cmd_r48(APP_CMD, ResponseType::R1, u32::from(rca) << 16)
}

/// CMD3: Send RCA
pub fn send_relative_address() -> Command {
    Command::no_data_cmd_r48(SEND_RCA, ResponseType::R6, 0)
}

/// CMD8: Sends memory card interface conditions
pub fn send_if_cond(voltage: u32, checkpattern: u32) -> Command {
    let arg = voltage << 8 | checkpattern;
    Command::no_data_cmd_r48(SEND_IF_COND, ResponseType::R7, arg)
}

pub fn set_bus_width(arg: u32) -> Command {
    Command::no_data_cmd_r48(ACMD_SET_BUS, ResponseType::R1, arg)
}

/// ACMD41: App Op Command
pub fn sd_send_op_cond(host_high_capacity_support: bool, sr18: bool) -> Command {
    let mut cmd = Command::default();
    let arg = u32::from(host_high_capacity_support) << 30 | u32::from(sr18) << 24 | 1 << 20;
    cmd.arg = arg;
    cmd.index = ACMD_SD_SEND_OP_COND;
    cmd.resp_ty = ResponseType::R3;
    cmd.reg_flags |= CmdMask::start_cmd.bits()
        | CmdMask::use_hold_reg.bits()
        | CmdMask::wait_prvdata_complete.bits()
        | CmdMask::response_expect.bits();
    cmd
}
