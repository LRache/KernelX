use core::{fmt::Debug, str};

#[non_exhaustive]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum SDSpecVersion {
    V1_0,
    V1_10,
    V2,
    V3,
    V4,
    V5,
    V6,
    V7,
    Unknown,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[allow(unused)]
pub enum BusWidth {
    #[non_exhaustive]
    Unknown,
    One = 1,
    Four = 4,
    Eight = 8,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum BlockSize {
    #[non_exhaustive]
    B1 = 0,
    B2 = 1,
    B4 = 2,
    B8 = 3,
    B16 = 4,
    B32 = 5,
    B64 = 6,
    B128 = 7,
    B256 = 8,
    B512 = 9,
    B1024 = 10,
    B2048 = 11,
    B4096 = 12,
    B8192 = 13,
    B16kB = 14,
    Unknown = 15,
}

#[derive(Copy, Clone, Eq, PartialEq)]
#[allow(non_camel_case_types)]
pub enum CurrentConsumption {
    I_0mA,
    I_1mA,
    I_5mA,
    I_10mA,
    I_25mA,
    I_35mA,
    I_45mA,
    I_60mA,
    I_80mA,
    I_100mA,
    I_200mA,
}

impl From<&CurrentConsumption> for u32 {
    fn from(i: &CurrentConsumption) -> u32 {
        match i {
            CurrentConsumption::I_0mA => 0,
            CurrentConsumption::I_1mA => 1,
            CurrentConsumption::I_5mA => 5,
            CurrentConsumption::I_10mA => 10,
            CurrentConsumption::I_25mA => 25,
            CurrentConsumption::I_35mA => 35,
            CurrentConsumption::I_45mA => 45,
            CurrentConsumption::I_60mA => 60,
            CurrentConsumption::I_80mA => 80,
            CurrentConsumption::I_100mA => 100,
            CurrentConsumption::I_200mA => 200,
        }
    }
}
impl CurrentConsumption {
    fn from_minimum_reg(reg: u128) -> CurrentConsumption {
        match reg & 0x7 {
            0 => CurrentConsumption::I_0mA,
            1 => CurrentConsumption::I_1mA,
            2 => CurrentConsumption::I_5mA,
            3 => CurrentConsumption::I_10mA,
            4 => CurrentConsumption::I_25mA,
            5 => CurrentConsumption::I_35mA,
            6 => CurrentConsumption::I_60mA,
            _ => CurrentConsumption::I_100mA,
        }
    }
    fn from_maximum_reg(reg: u128) -> CurrentConsumption {
        match reg & 0x7 {
            0 => CurrentConsumption::I_1mA,
            1 => CurrentConsumption::I_5mA,
            2 => CurrentConsumption::I_10mA,
            3 => CurrentConsumption::I_25mA,
            4 => CurrentConsumption::I_35mA,
            5 => CurrentConsumption::I_45mA,
            6 => CurrentConsumption::I_80mA,
            _ => CurrentConsumption::I_200mA,
        }
    }
}
impl Debug for CurrentConsumption {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let ma: u32 = self.into();
        write!(f, "{} mA", ma)
    }
}

#[derive(Eq, PartialEq, Copy, Clone, Debug)]
#[allow(dead_code)]
pub enum CurrentState {
    Ready = 1,
    Identification = 2,
    Standby = 3,
    Transfer = 4,
    Sending = 5,
    Receiving = 6,
    Programming = 7,
    Disconnected = 8,
    BusTest = 9,
    Sleep = 10,
    Error = 128,
}

impl From<u8> for CurrentState {
    fn from(n: u8) -> Self {
        match n {
            1 => Self::Ready,
            2 => Self::Identification,
            3 => Self::Standby,
            4 => Self::Transfer,
            5 => Self::Sending,
            6 => Self::Receiving,
            7 => Self::Programming,
            8 => Self::Disconnected,
            9 => Self::BusTest,
            10 => Self::Sleep,
            _ => Self::Error,
        }
    }
}
#[derive(Copy, Clone, Default)]
pub struct Ocr(u32);
impl From<u32> for Ocr {
    fn from(value: u32) -> Self {
        Self(value)
    }
}
impl Ocr {
    pub fn is_busy(&self) -> bool {
        self.0 & 0x8000_0000 == 0
    }
    pub fn voltage_window_mv(&self) -> Option<(u16, u16)> {
        let mut window = (self.0 >> 15) & 0x1FF;
        let mut min = 2_700;

        while window & 1 == 0 && window != 0 {
            min += 100;
            window >>= 1;
        }
        let mut max = min;
        while window != 0 {
            max += 100;
            window >>= 1;
        }

        if max == min {
            None
        } else {
            Some((min, max))
        }
    }

    pub fn v18_allowed(&self) -> bool {
        self.0 & 0x0100_0000 != 0
    }

    pub fn over_2tb(&self) -> bool {
        self.0 & 0x0800_0000 != 0
    }

    pub fn uhs2_card_status(&self) -> bool {
        self.0 & 0x2000_0000 != 0
    }

    pub fn high_capacity(&self) -> bool {
        self.0 & 0x4000_0000 != 0
    }
}

impl Debug for Ocr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OCR: Operation Conditions Register")
            .field(
                "Voltage Window (mV)",
                &self.voltage_window_mv().unwrap_or((0, 0)),
            )
            .field("S18A (UHS-I only)", &self.v18_allowed())
            .field("Over 2TB flag (SDUC only)", &self.over_2tb())
            .field("UHS-II Card", &self.uhs2_card_status())
            .field(
                "Card Capacity Status (CSS)",
                &if self.high_capacity() {
                    "SDHC/SDXC/SDUC"
                } else {
                    "SDSC"
                },
            )
            .field("Busy", &self.is_busy())
            .finish()
    }
}
#[derive(Copy, Clone, Default)]
pub struct Cid {
    inner: u128,
    bytes: [u8; 16],
}
impl From<(u32, u32, u32, u32)> for Cid {
    fn from(value: (u32, u32, u32, u32)) -> Self {
        let inner = (value.3 as u128) << 96
            | (value.2 as u128) << 64
            | (value.1 as u128) << 32
            | (value.0 as u128);
        Self {
            inner,
            bytes: inner.to_be_bytes(),
        }
    }
}

impl From<u128> for Cid {
    fn from(value: u128) -> Self {
        Self {
            inner: value,
            bytes: value.to_be_bytes(),
        }
    }
}

impl Cid {
    pub fn manufacturer_id(&self) -> u8 {
        self.bytes[0]
    }
    #[allow(unused)]
    pub fn crc7(&self) -> u8 {
        (self.bytes[15] >> 1) & 0x7F
    }

    pub fn oem_id(&self) -> &str {
        str::from_utf8(&self.bytes[1..3]).unwrap_or(&"<ERR>")
    }

    pub fn product_name(&self) -> &str {
        str::from_utf8(&self.bytes[3..8]).unwrap_or(&"<ERR>")
    }

    pub fn product_revision(&self) -> u8 {
        self.bytes[8]
    }

    pub fn serial(&self) -> u32 {
        (self.inner >> 24) as u32
    }

    pub fn manufacturing_date(&self) -> (u8, u16) {
        (
            (self.inner >> 8) as u8 & 0xF,             // Month
            ((self.inner >> 12) as u16 & 0xFF) + 2000, // Year
        )
    }
}

impl Debug for Cid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CID: Card Identification")
            .field("Manufacturer ID", &self.manufacturer_id())
            .field("OEM ID", &self.oem_id())
            .field("Product Name", &self.product_name())
            .field("Product Revision", &self.product_revision())
            .field("Product Serial Number", &self.serial())
            .field("Manufacturing Date", &self.manufacturing_date())
            .finish()
    }
}
#[derive(Copy, Clone, Default)]
pub struct Csd(u128);
impl From<(u32, u32, u32, u32)> for Csd {
    fn from(value: (u32, u32, u32, u32)) -> Self {
        let inner = (value.3 as u128) << 96
            | (value.2 as u128) << 64
            | (value.1 as u128) << 32
            | (value.0 as u128);
        Self(inner)
    }
}

impl Csd {
    pub fn version(&self) -> u8 {
        (self.0 >> 126) as u8 & 3
    }

    pub fn transfer_rate(&self) -> u8 {
        (self.0 >> 96) as u8
    }

    pub fn block_length(&self) -> BlockSize {
        // Read block length
        match (self.0 >> 80) & 0xF {
            0 => BlockSize::B1,
            1 => BlockSize::B2,
            2 => BlockSize::B4,
            3 => BlockSize::B8,
            4 => BlockSize::B16,
            5 => BlockSize::B32,
            6 => BlockSize::B64,
            7 => BlockSize::B128,
            8 => BlockSize::B256,
            9 => BlockSize::B512,
            10 => BlockSize::B1024,
            11 => BlockSize::B2048,
            12 => BlockSize::B4096,
            13 => BlockSize::B8192,
            14 => BlockSize::B16kB,
            _ => BlockSize::Unknown,
        }
    }

    pub fn read_current_minimum_vdd(&self) -> CurrentConsumption {
        CurrentConsumption::from_minimum_reg((self.0 >> 59) & 0x7)
    }

    pub fn write_current_minimum_vdd(&self) -> CurrentConsumption {
        CurrentConsumption::from_minimum_reg((self.0 >> 56) & 0x7)
    }

    pub fn read_current_maximum_vdd(&self) -> CurrentConsumption {
        CurrentConsumption::from_maximum_reg((self.0 >> 53) & 0x7)
    }

    pub fn write_current_maximum_vdd(&self) -> CurrentConsumption {
        CurrentConsumption::from_maximum_reg((self.0 >> 50) & 0x7)
    }

    pub fn block_count(&self) -> u64 {
        match self.version() {
            0 => {
                // SDSC
                let c_size: u16 = ((self.0 >> 62) as u16) & 0xFFF;
                let c_size_mult: u8 = ((self.0 >> 47) as u8) & 7;

                ((c_size + 1) as u64) * ((1 << (c_size_mult + 2)) as u64)
            }
            1 => {
                // SDHC/SDXC
                (((self.0 >> 48) as u64 & 0x3F_FFFF) + 1) * 1024
            }
            2 => {
                // SDUC
                (((self.0 >> 48) as u64 & 0xFFF_FFFF) + 1) * 1024
            }
            _ => 0,
        }
    }

    pub fn card_size(&self) -> u64 {
        let block_size_bytes = 1 << self.block_length() as u64;

        self.block_count() * block_size_bytes
    }

    pub fn erase_size_blocks(&self) -> u32 {
        if (self.0 >> 46) & 1 == 1 {
            // ERASE_BLK_EN
            1
        } else {
            let sector_size_tens = (self.0 >> 43) & 0x7;
            let sector_size_units = (self.0 >> 39) & 0xF;

            (sector_size_tens as u32 * 10) + (sector_size_units as u32)
        }
    }
}

impl Debug for Csd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CSD: Card Specific Data")
            .field("Transfer Rate", &self.transfer_rate())
            .field("Block Count", &self.block_count())
            .field("Card Size (bytes)", &self.card_size())
            .field("Read I (@min VDD)", &self.read_current_minimum_vdd())
            .field("Write I (@min VDD)", &self.write_current_minimum_vdd())
            .field("Read I (@max VDD)", &self.read_current_maximum_vdd())
            .field("Write I (@max VDD)", &self.write_current_maximum_vdd())
            .field("Erase Size (Blocks)", &self.erase_size_blocks())
            .finish()
    }
}
#[derive(Copy, Clone, Default)]
pub struct CardStatus(u32);
impl From<u32> for CardStatus {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl CardStatus {
    pub fn ecc_disabled(&self) -> bool {
        self.0 & 0x4000 != 0
    }

    pub fn fx_event(&self) -> bool {
        self.0 & 0x40 != 0
    }

    pub fn ake_seq_error(&self) -> bool {
        self.0 & 0x8 != 0
    }

    pub fn out_of_range(&self) -> bool {
        self.0 & 0x8000_0000 != 0
    }

    pub fn address_error(&self) -> bool {
        self.0 & 0x4000_0000 != 0
    }

    pub fn block_len_error(&self) -> bool {
        self.0 & 0x2000_0000 != 0
    }

    pub fn erase_seq_error(&self) -> bool {
        self.0 & 0x1000_0000 != 0
    }

    pub fn erase_param(&self) -> bool {
        self.0 & 0x800_0000 != 0
    }

    pub fn wp_violation(&self) -> bool {
        self.0 & 0x400_0000 != 0
    }

    pub fn card_is_locked(&self) -> bool {
        self.0 & 0x200_0000 != 0
    }

    pub fn lock_unlock_failed(&self) -> bool {
        self.0 & 0x100_0000 != 0
    }

    pub fn com_crc_error(&self) -> bool {
        self.0 & 0x80_0000 != 0
    }

    pub fn illegal_command(&self) -> bool {
        self.0 & 0x40_0000 != 0
    }

    pub fn card_ecc_failed(&self) -> bool {
        self.0 & 0x20_0000 != 0
    }

    pub fn cc_error(&self) -> bool {
        self.0 & 0x10_0000 != 0
    }

    pub fn error(&self) -> bool {
        self.0 & 0x8_0000 != 0
    }

    pub fn csd_overwrite(&self) -> bool {
        self.0 & 0x1_0000 != 0
    }

    pub fn wp_erase_skip(&self) -> bool {
        self.0 & 0x8000 != 0
    }

    pub fn erase_reset(&self) -> bool {
        self.0 & 0x2000 != 0
    }

    pub fn state(&self) -> CurrentState {
        CurrentState::from(((self.0 >> 9) & 0xF) as u8)
    }

    pub fn ready_for_data(&self) -> bool {
        self.0 & 0x100 != 0
    }

    pub fn app_cmd(&self) -> bool {
        self.0 & 0x20 != 0
    }
}
impl Debug for CardStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Card Status")
            .field("Out of range error", &self.out_of_range())
            .field("Address error", &self.address_error())
            .field("Block len error", &self.block_len_error())
            .field("Erase seq error", &self.erase_seq_error())
            .field("Erase param error", &self.erase_param())
            .field("Write protect error", &self.wp_violation())
            .field("Card locked", &self.card_is_locked())
            .field("Password lock unlock error", &self.lock_unlock_failed())
            .field(
                "Crc check for the previous command failed",
                &self.com_crc_error(),
            )
            .field("Illegal command", &self.illegal_command())
            .field("Card internal ecc failed", &self.card_ecc_failed())
            .field("Internal card controller error", &self.cc_error())
            .field("General Error", &self.error())
            .field("Csd error", &self.csd_overwrite())
            .field("Write protect error", &self.wp_erase_skip())
            .field("Command ecc disabled", &self.ecc_disabled())
            .field("Erase sequence cleared", &self.erase_reset())
            .field("Card state", &self.state())
            .field("Buffer empty", &self.ready_for_data())
            .field("Extension event", &self.fx_event())
            .field("Card expects app cmd", &self.app_cmd())
            .field("Auth process error", &self.ake_seq_error())
            .finish()
    }
}
#[derive(Copy, Clone, Default)]
pub struct Rca(u32);
impl From<u32> for Rca {
    fn from(value: u32) -> Self {
        Self(value)
    }
}
impl Rca {
    pub fn address(&self) -> u16 {
        (self.0 >> 16) as u16
    }

    pub fn status(&self) -> u16 {
        self.0 as u16
    }
}

impl Debug for Rca {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Rca")
            .field("address", &self.address())
            .field("status", &self.status())
            .finish()
    }
}

#[derive(Copy, Clone, Default)]
pub struct Scr(u64);
impl From<(u32, u32)> for Scr {
    fn from(value: (u32, u32)) -> Self {
        let val = (value.1 as u64) << 32 | (value.0 as u64);
        Self(val)
    }
}

impl Scr {
    pub fn version(&self) -> SDSpecVersion {
        let spec = (self.0 >> 56) & 0xF;
        let spec3 = (self.0 >> 47) & 1;
        let spec4 = (self.0 >> 42) & 1;
        let specx = (self.0 >> 38) & 0xF;

        // Ref PLSS_v7_10 Table 5-17
        match (spec, spec3, spec4, specx) {
            (0, 0, 0, 0) => SDSpecVersion::V1_0,
            (1, 0, 0, 0) => SDSpecVersion::V1_10,
            (2, 0, 0, 0) => SDSpecVersion::V2,
            (2, 1, 0, 0) => SDSpecVersion::V3,
            (2, 1, 1, 0) => SDSpecVersion::V4,
            (2, 1, _, 1) => SDSpecVersion::V5,
            (2, 1, _, 2) => SDSpecVersion::V6,
            (2, 1, _, 3) => SDSpecVersion::V7,
            _ => SDSpecVersion::Unknown,
        }
    }

    #[allow(unused)]
    pub fn bus_widths(&self) -> u8 {
        // Ref PLSS_v7_10 Table 5-21
        ((self.0 >> 48) as u8) & 0xF
    }

    pub fn bus_width_one(&self) -> bool {
        (self.0 >> 48) & 1 != 0
    }

    pub fn bus_width_four(&self) -> bool {
        (self.0 >> 50) & 1 != 0
    }
}

impl Debug for Scr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SCR: SD CARD Configuration Register")
            .field("Version", &self.version())
            .field("1-bit width", &self.bus_width_one())
            .field("4-bit width", &self.bus_width_four())
            .finish()
    }
}
#[derive(Copy, Clone, Default)]
pub struct Cic(u32);

impl From<u32> for Cic {
    fn from(word: u32) -> Self {
        Self(word)
    }
}

impl Cic {
    pub fn voltage_accepted(&self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub fn pattern(&self) -> u8 {
        self.0 as u8
    }
}

#[derive(Clone, Copy, Default)]
pub struct SdStatus {
    inner: [u32; 16],
}

impl From<[u32; 16]> for SdStatus {
    fn from(value: [u32; 16]) -> Self {
        Self { inner: value }
    }
}

impl SdStatus {
    pub fn bus_width(&self) -> BusWidth {
        match (self.inner[15] >> 30) & 3 {
            0 => BusWidth::One,
            2 => BusWidth::Four,
            _ => BusWidth::Unknown,
        }
    }

    pub fn secure_mode(&self) -> bool {
        self.inner[15] & 0x2000_0000 != 0
    }

    pub fn sd_memory_card_type(&self) -> u16 {
        self.inner[15] as u16
    }

    pub fn protected_area_size(&self) -> u32 {
        self.inner[14]
    }

    pub fn speed_class(&self) -> u8 {
        (self.inner[13] >> 24) as u8
    }

    pub fn move_performance(&self) -> u8 {
        (self.inner[13] >> 16) as u8
    }

    pub fn allocation_unit_size(&self) -> u8 {
        (self.inner[13] >> 12) as u8 & 0xF
    }

    pub fn erase_size(&self) -> u16 {
        (self.inner[13] & 0xFF) as u16 | ((self.inner[12] >> 24) & 0xFF) as u16
    }

    pub fn erase_timeout(&self) -> u8 {
        (self.inner[12] >> 18) as u8 & 0x3F
    }

    pub fn video_speed_class(&self) -> u8 {
        (self.inner[11] & 0xFF) as u8
    }

    pub fn app_perf_class(&self) -> u8 {
        (self.inner[9] >> 16) as u8 & 0xF
    }

    pub fn discard_support(&self) -> bool {
        self.inner[8] & 0x0200_0000 != 0
    }
}
impl Debug for SdStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SD Status")
            .field("Bus Width", &self.bus_width())
            .field("Secured Mode", &self.secure_mode())
            .field("SD Memory Card Type", &self.sd_memory_card_type())
            .field("Protected Area Size (B)", &self.protected_area_size())
            .field("Speed Class", &self.speed_class())
            .field("Video Speed Class", &self.video_speed_class())
            .field("Application Performance Class", &self.app_perf_class())
            .field("Move Performance (MB/s)", &self.move_performance())
            .field("AU Size", &self.allocation_unit_size())
            .field("Erase Size (units of AU)", &self.erase_size())
            .field("Erase Timeout (s)", &self.erase_timeout())
            .field("Discard Support", &self.discard_support())
            .finish()
    }
}
