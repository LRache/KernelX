use core::mem::{align_of, size_of};

use crate::arch;
use crate::klib::InitedCell;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemRegion {
    pub start: usize,
    pub end: usize,
}

pub const MAX_MEM_REGIONS: usize = arch::PGSIZE / size_of::<MemRegion>();

static MEM_REGIONS: InitedCell<&'static [MemRegion]> = InitedCell::uninit();

/// # Safety
///
/// `regions` must point to `count` initialized entries in the bootstrap memory
/// region page. That page must remain reserved for the lifetime of the kernel.
pub unsafe fn init(regions: *const MemRegion, count: usize) -> &'static [MemRegion] {
    assert_eq!(arch::PGSIZE, 4096, "boot memory region ABI requires 4 KiB pages");
    assert!(!regions.is_null(), "boot memory region array is null");
    assert!(count > 0, "boot memory region array is empty");
    assert!(count <= MAX_MEM_REGIONS, "too many boot memory regions");
    assert_eq!(regions as usize % align_of::<MemRegion>(), 0);

    // SAFETY: The caller guarantees that the bootstrap page contains `count`
    // initialized entries and remains reserved for the kernel lifetime.
    let regions = unsafe { core::slice::from_raw_parts(regions, count) };

    let mut previous_end = 0;
    for (index, region) in regions.iter().enumerate() {
        assert!(region.start < region.end, "empty boot memory region");
        assert_eq!(region.start % arch::PGSIZE, 0, "unaligned memory region start");
        assert_eq!(region.end % arch::PGSIZE, 0, "unaligned memory region end");
        if index > 0 {
            assert!(previous_end < region.start, "overlapping boot memory regions");
        }
        previous_end = region.end;
    }

    MEM_REGIONS.init(regions);
    regions
}

pub fn regions() -> &'static [MemRegion] {
    *MEM_REGIONS
}

pub fn max_end() -> usize {
    regions().last().expect("boot memory regions are not initialized").end
}

pub fn contains_range(paddr: usize, len: usize) -> bool {
    let Some(end) = paddr.checked_add(len) else {
        return false;
    };
    len != 0
        && regions()
            .iter()
            .any(|region| paddr >= region.start && end <= region.end)
}
