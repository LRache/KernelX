/// 运行时符号查找（仅 Debug 模式）。
///
/// 二进制格式（由 scripts/gen_symbols.py 生成）：
///   [u32 LE: count]
///   [count * { u64 LE: addr, u32 LE: name_offset }]
///   [string pool: null-terminated UTF-8]
///

#[cfg(debug_assertions)]
static SYMBOL_BYTES: &[u8] = include_bytes!(env!("KERNELX_SYMBOLS_PATH"));

/// 查找 `pc` 所属的函数名及偏移量。
/// 返回 `(function_name, pc - function_start)`。
#[cfg(debug_assertions)]
pub fn lookup(pc: usize) -> Option<(&'static str, usize)> {
    let data = SYMBOL_BYTES;
    if data.len() < 4 {
        return None;
    }
    let count = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
    if count == 0 {
        return None;
    }
    let entries_end = 4 + count * 12;
    if data.len() < entries_end {
        return None;
    }
    let entries = &data[4..entries_end];
    let string_pool = &data[entries_end..];

    let addr_at = |i: usize| -> usize {
        u64::from_le_bytes(entries[i * 12..i * 12 + 8].try_into().unwrap_or([0; 8])) as usize
    };

    if addr_at(0) > pc {
        return None;
    }

    let mut lo = 0usize;
    let mut hi = count - 1;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if addr_at(mid) <= pc {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let func_addr = addr_at(lo);
    let name_offset = u32::from_le_bytes(
        entries[lo * 12 + 8..lo * 12 + 12].try_into().ok()?
    ) as usize;

    if name_offset >= string_pool.len() {
        return None;
    }
    let name_bytes = &string_pool[name_offset..];
    let len = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
    let name = core::str::from_utf8(&name_bytes[..len]).ok()?;

    Some((name, pc - func_addr))
}

#[cfg(not(debug_assertions))]
#[inline(always)]
pub fn lookup(_pc: usize) -> Option<(&'static str, usize)> {
    None
}
