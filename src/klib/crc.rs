pub const CRC32C_INIT: u32 = 0xffff_ffff;

const CRC32C_TABLE: [[u32; 256]; 8] = {
    const CRC32C_POLY_REVERSED: u32 = 0x82F63B78;

    let mut table = [[0u32; 256]; 8];

    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ CRC32C_POLY_REVERSED
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[0][i] = crc;
        i += 1;
    }

    let mut s = 1;
    while s < 8 {
        let mut i = 0;
        while i < 256 {
            let prev = table[s - 1][i];
            table[s][i] = (prev >> 8) ^ table[0][(prev & 0xFF) as usize];
            i += 1;
        }
        s += 1;
    }

    table
};

/// Update CRC32C(Castagnoli) with `data` from an existing `crc` state.
#[inline]
pub fn crc32c_update(crc: u32, data: &[u8]) -> u32 {
    crate::arch::crc32c(crc, data)
}

/// Architecture-independent CRC32C fallback.
///
/// Uses slicing-by-8 for bulk processing, falls back to byte-at-a-time for
/// the remaining tail bytes.
#[inline]
pub(crate) fn crc32c_update_generic(mut crc: u32, data: &[u8]) -> u32 {
    let mut i = 0;
    let len = data.len();

    while i + 8 <= len {
        crc ^= u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let b4 = data[i + 4];
        let b5 = data[i + 5];
        let b6 = data[i + 6];
        let b7 = data[i + 7];

        crc = CRC32C_TABLE[7][(crc & 0xFF) as usize]
            ^ CRC32C_TABLE[6][((crc >> 8) & 0xFF) as usize]
            ^ CRC32C_TABLE[5][((crc >> 16) & 0xFF) as usize]
            ^ CRC32C_TABLE[4][((crc >> 24) & 0xFF) as usize]
            ^ CRC32C_TABLE[3][b4 as usize]
            ^ CRC32C_TABLE[2][b5 as usize]
            ^ CRC32C_TABLE[1][b6 as usize]
            ^ CRC32C_TABLE[0][b7 as usize];

        i += 8;
    }

    while i < len {
        let idx = (crc as u8) ^ data[i];
        crc = CRC32C_TABLE[0][idx as usize] ^ (crc >> 8);
        i += 1;
    }

    crc
}

const CRC16_TABLE: [[u16; 256]; 8] = {
    const CRC16_POLY_REVERSED: u16 = 0xA001;

    let mut table = [[0u16; 256]; 8];

    let mut i = 0;
    while i < 256 {
        let mut crc = i as u16;
        let mut bit = 0;
        while bit < 8 {
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ CRC16_POLY_REVERSED
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[0][i] = crc;
        i += 1;
    }

    let mut s = 1;
    while s < 8 {
        let mut i = 0;
        while i < 256 {
            let prev = table[s - 1][i];
            table[s][i] = (prev >> 8) ^ table[0][(prev & 0xFF) as usize];
            i += 1;
        }
        s += 1;
    }

    table
};

/// Update CRC16 (poly 0x8005, reflected) with `data` from an existing `crc` state.
///
/// Uses slicing-by-8 for bulk processing, falls back to byte-at-a-time for
/// the remaining tail bytes.
#[inline]
pub fn crc16_update(mut crc: u16, data: &[u8]) -> u16 {
    let mut i = 0;
    let len = data.len();

    while i + 8 <= len {
        let b0 = data[i] ^ (crc as u8);
        let b1 = data[i + 1] ^ ((crc >> 8) as u8);
        let b2 = data[i + 2];
        let b3 = data[i + 3];
        let b4 = data[i + 4];
        let b5 = data[i + 5];
        let b6 = data[i + 6];
        let b7 = data[i + 7];

        crc = CRC16_TABLE[7][b0 as usize]
            ^ CRC16_TABLE[6][b1 as usize]
            ^ CRC16_TABLE[5][b2 as usize]
            ^ CRC16_TABLE[4][b3 as usize]
            ^ CRC16_TABLE[3][b4 as usize]
            ^ CRC16_TABLE[2][b5 as usize]
            ^ CRC16_TABLE[1][b6 as usize]
            ^ CRC16_TABLE[0][b7 as usize];

        i += 8;
    }

    // Handle the remaining tail shorter than eight bytes.
    while i < len {
        let idx = (crc as u8) ^ data[i];
        crc = CRC16_TABLE[0][idx as usize] ^ (crc >> 8);
        i += 1;
    }

    crc
}
