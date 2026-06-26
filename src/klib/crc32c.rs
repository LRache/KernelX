pub const CRC32C_INIT: u32 = 0xffff_ffff;

const CRC32C_POLY: u32 = 0x82f6_3b78;
const CRC32C_TABLE_COUNT: usize = 8;
const CRC32C_TABLE_SIZE: usize = 256;
const CRC32C_TABLES: [[u32; CRC32C_TABLE_SIZE]; CRC32C_TABLE_COUNT] = make_crc32c_tables();

const fn make_crc32c_tables() -> [[u32; CRC32C_TABLE_SIZE]; CRC32C_TABLE_COUNT] {
    let mut tables = [[0u32; CRC32C_TABLE_SIZE]; CRC32C_TABLE_COUNT];
    let mut index = 0;
    while index < CRC32C_TABLE_SIZE {
        tables[0][index] = make_crc32c_entry(index as u32);
        index += 1;
    }

    let mut table = 1;
    while table < CRC32C_TABLE_COUNT {
        let mut index = 0;
        while index < CRC32C_TABLE_SIZE {
            let previous = tables[table - 1][index];
            tables[table][index] = tables[0][(previous & 0xff) as usize] ^ (previous >> 8);
            index += 1;
        }
        table += 1;
    }

    tables
}

const fn make_crc32c_entry(mut value: u32) -> u32 {
    let mut bit = 0;
    while bit < 8 {
        if value & 1 != 0 {
            value = (value >> 1) ^ CRC32C_POLY;
        } else {
            value >>= 1;
        }
        bit += 1;
    }
    value
}

pub fn default_crc32c(seed: u32, buf: &[u8]) -> u32 {
    let mut crc = seed;
    let mut offset = 0;
    while offset + 8 <= buf.len() {
        let value = crc ^ u32::from_le_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]]);
        let next = u32::from_le_bytes([buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7]]);
        crc = CRC32C_TABLES[7][(value & 0xff) as usize]
            ^ CRC32C_TABLES[6][((value >> 8) & 0xff) as usize]
            ^ CRC32C_TABLES[5][((value >> 16) & 0xff) as usize]
            ^ CRC32C_TABLES[4][(value >> 24) as usize]
            ^ CRC32C_TABLES[3][(next & 0xff) as usize]
            ^ CRC32C_TABLES[2][((next >> 8) & 0xff) as usize]
            ^ CRC32C_TABLES[1][((next >> 16) & 0xff) as usize]
            ^ CRC32C_TABLES[0][(next >> 24) as usize];
        offset += 8;
    }

    while offset < buf.len() {
        crc = CRC32C_TABLES[0][((crc as u8) ^ buf[offset]) as usize] ^ (crc >> 8);
        offset += 1;
    }
    crc
}
