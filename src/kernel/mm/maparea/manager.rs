use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
#[cfg(feature = "map-manager-lock-debug")]
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::arch::{self, PageTable};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::anonymous::PrivateAnonymousArea;
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::klib::{SleepLockOnStack, SpinLock};
#[cfg(feature = "map-manager-lock-debug")]
use crate::println;
use crate::{ktrace, print};

use super::area::{Area, MapAreaInfo, MemoryFaultError, PinPageFrame};
use super::userbrk::UserBrk;
use super::userstack::{Auxv, UserStack};
use super::watcher::{MapChange, MapChangeEvent, MapChangeNotifier, MapManagerWatcher};

// PERF_DEBUG_BEGIN(map-manager-lock): Temporary lock attribution, per-Area
// aggregation, and slow-event capture. Remove this whole block together with
// the PERF_DEBUG(map-manager-lock) call sites below.
#[cfg(feature = "map-manager-lock-debug")]
mod perf_debug {
    use super::*;

    struct DebugPhaseStats {
        total_us: AtomicU64,
        max_us: AtomicU64,
        samples: AtomicU64,
        sub_us_samples: AtomicU64,
    }

    impl DebugPhaseStats {
        const fn new() -> Self {
            Self {
                total_us: AtomicU64::new(0),
                max_us: AtomicU64::new(0),
                samples: AtomicU64::new(0),
                sub_us_samples: AtomicU64::new(0),
            }
        }

        fn record(&self, elapsed_us: u64) {
            self.total_us.fetch_add(elapsed_us, Ordering::Relaxed);
            self.max_us.fetch_max(elapsed_us, Ordering::Relaxed);
            self.samples.fetch_add(1, Ordering::Relaxed);
            if elapsed_us == 0 {
                self.sub_us_samples.fetch_add(1, Ordering::Relaxed);
            }
        }

        fn dump(&self, name: &str) {
            let total_us = self.total_us.load(Ordering::Relaxed);
            let samples = self.samples.load(Ordering::Relaxed);
            let average_milli_us = total_us.saturating_mul(1000) / samples.max(1);
            println!(
                "    {:<9} total={} us avg={}.{:03} us max={} us samples={} sub-us={}",
                name,
                total_us,
                average_milli_us / 1000,
                average_milli_us % 1000,
                self.max_us.load(Ordering::Relaxed),
                samples,
                self.sub_us_samples.load(Ordering::Relaxed),
            );
        }
    }

    struct DebugOperationStats {
        calls: AtomicU64,
        total_us: AtomicU64,
        max_us: AtomicU64,
        area_set: DebugPhaseStats,
        area_data: DebugPhaseStats,
    }

    impl DebugOperationStats {
        const fn new() -> Self {
            Self {
                calls: AtomicU64::new(0),
                total_us: AtomicU64::new(0),
                max_us: AtomicU64::new(0),
                area_set: DebugPhaseStats::new(),
                area_data: DebugPhaseStats::new(),
            }
        }

        fn record(&self, elapsed_us: u64) {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.total_us.fetch_add(elapsed_us, Ordering::Relaxed);
            self.max_us.fetch_max(elapsed_us, Ordering::Relaxed);
        }

        fn dump(&self, name: &str) {
            let calls = self.calls.load(Ordering::Relaxed);
            let total_us = self.total_us.load(Ordering::Relaxed);
            let average_milli_us = total_us.saturating_mul(1000) / calls.max(1);
            let classified_us = self
                .area_set
                .total_us
                .load(Ordering::Relaxed)
                .saturating_add(self.area_data.total_us.load(Ordering::Relaxed));
            println!(
                "  {}: calls={} total={} us avg={}.{:03} us max={} us unclassified={} us",
                name,
                calls,
                total_us,
                average_milli_us / 1000,
                average_milli_us % 1000,
                self.max_us.load(Ordering::Relaxed),
                total_us.saturating_sub(classified_us),
            );
            self.area_set.dump("area-set");
            self.area_data.dump("area-data");
        }
    }

    struct DebugAreaKindStats {
        translate_read: DebugPhaseStats,
        translate_write: DebugPhaseStats,
        get_frame: DebugPhaseStats,
        memory_fault: DebugPhaseStats,
    }

    impl DebugAreaKindStats {
        const fn new() -> Self {
            Self {
                translate_read: DebugPhaseStats::new(),
                translate_write: DebugPhaseStats::new(),
                get_frame: DebugPhaseStats::new(),
                memory_fault: DebugPhaseStats::new(),
            }
        }

        fn operation(&self, operation: DebugOperation) -> &DebugPhaseStats {
            match operation {
                DebugOperation::TranslateRead => &self.translate_read,
                DebugOperation::TranslateWrite => &self.translate_write,
                DebugOperation::GetFrame => &self.get_frame,
                DebugOperation::MemoryFault => &self.memory_fault,
            }
        }

        fn dump(&self, name: &str) {
            let samples = self
                .translate_read
                .samples
                .load(Ordering::Relaxed)
                .saturating_add(self.translate_write.samples.load(Ordering::Relaxed))
                .saturating_add(self.get_frame.samples.load(Ordering::Relaxed))
                .saturating_add(self.memory_fault.samples.load(Ordering::Relaxed));
            if samples == 0 {
                return;
            }

            println!("  {}:", name);
            self.translate_read.dump("read");
            self.translate_write.dump("write");
            self.get_frame.dump("get-frame");
            self.memory_fault.dump("fault");
        }
    }

    #[derive(Clone, Copy)]
    #[repr(usize)]
    enum DebugAreaKind {
        Unknown,
        PrivateAnonymous,
        SharedAnonymous,
        PrivateFile,
        SharedFile,
        Elf,
        Stack,
        Shm,
    }

    impl DebugAreaKind {
        fn from_name(name: &str) -> Self {
            match name {
                "private-anonymous" => Self::PrivateAnonymous,
                "shared-anonymous" => Self::SharedAnonymous,
                "PrivateFileMapArea" => Self::PrivateFile,
                "SharedFileMapArea" => Self::SharedFile,
                "elf" => Self::Elf,
                "stack" => Self::Stack,
                "ShmArea" => Self::Shm,
                _ => Self::Unknown,
            }
        }

        fn from_raw(value: usize) -> Self {
            match value {
                value if value == Self::PrivateAnonymous as usize => Self::PrivateAnonymous,
                value if value == Self::SharedAnonymous as usize => Self::SharedAnonymous,
                value if value == Self::PrivateFile as usize => Self::PrivateFile,
                value if value == Self::SharedFile as usize => Self::SharedFile,
                value if value == Self::Elf as usize => Self::Elf,
                value if value == Self::Stack as usize => Self::Stack,
                value if value == Self::Shm as usize => Self::Shm,
                _ => Self::Unknown,
            }
        }

        fn name(self) -> &'static str {
            match self {
                Self::Unknown => "unknown",
                Self::PrivateAnonymous => "private-anonymous",
                Self::SharedAnonymous => "shared-anonymous",
                Self::PrivateFile => "PrivateFileMapArea",
                Self::SharedFile => "SharedFileMapArea",
                Self::Elf => "elf",
                Self::Stack => "stack",
                Self::Shm => "ShmArea",
            }
        }

        fn stats(self) -> &'static DebugAreaKindStats {
            match self {
                Self::Unknown => &DEBUG_AREA_UNKNOWN,
                Self::PrivateAnonymous => &DEBUG_AREA_PRIVATE_ANONYMOUS,
                Self::SharedAnonymous => &DEBUG_AREA_SHARED_ANONYMOUS,
                Self::PrivateFile => &DEBUG_AREA_PRIVATE_FILE,
                Self::SharedFile => &DEBUG_AREA_SHARED_FILE,
                Self::Elf => &DEBUG_AREA_ELF,
                Self::Stack => &DEBUG_AREA_STACK,
                Self::Shm => &DEBUG_AREA_SHM,
            }
        }
    }

    #[derive(Clone, Copy)]
    struct DebugAreaIdentity {
        kind: DebugAreaKind,
        addrspace: usize,
        area: usize,
        backing: usize,
        ubase: usize,
        uaddr: usize,
    }

    impl DebugAreaIdentity {
        fn new(area: &mut dyn Area, addrspace: &AddrSpace, uaddr: usize) -> Self {
            Self {
                kind: DebugAreaKind::from_name(area.type_name()),
                addrspace: addrspace as *const AddrSpace as usize,
                area: area as *mut dyn Area as *mut () as usize,
                backing: area.debug_backing_id(),
                ubase: area.ubase(),
                uaddr,
            }
        }
    }

    #[derive(Clone, Copy)]
    #[repr(usize)]
    pub(super) enum DebugOperation {
        TranslateRead,
        TranslateWrite,
        GetFrame,
        MemoryFault,
    }

    impl DebugOperation {
        fn stats(self) -> &'static DebugOperationStats {
            match self {
                Self::TranslateRead => &DEBUG_TRANSLATE_READ,
                Self::TranslateWrite => &DEBUG_TRANSLATE_WRITE,
                Self::GetFrame => &DEBUG_GET_FRAME,
                Self::MemoryFault => &DEBUG_MEMORY_FAULT,
            }
        }

        fn from_raw(value: usize) -> Self {
            match value {
                value if value == Self::TranslateRead as usize => Self::TranslateRead,
                value if value == Self::TranslateWrite as usize => Self::TranslateWrite,
                value if value == Self::GetFrame as usize => Self::GetFrame,
                _ => Self::MemoryFault,
            }
        }

        fn name(self) -> &'static str {
            match self {
                Self::TranslateRead => "translate-read",
                Self::TranslateWrite => "translate-write",
                Self::GetFrame => "get-frame",
                Self::MemoryFault => "memory-fault",
            }
        }
    }

    pub(super) struct DebugOperationGuard {
        operation: DebugOperation,
        stats: &'static DebugOperationStats,
        start_us: u64,
    }

    impl DebugOperationGuard {
        pub(super) fn new(operation: DebugOperation) -> Self {
            Self {
                stats: operation.stats(),
                operation,
                start_us: arch::get_time_us(),
            }
        }

        pub(super) fn area_set(&self) -> DebugPhaseGuard<'_> {
            DebugPhaseGuard::new(&self.stats.area_set)
        }

        pub(super) fn area_data<'a>(
            &'a self,
            area: &mut dyn Area,
            addrspace: &AddrSpace,
            uaddr: usize,
        ) -> DebugAreaDataGuard<'a> {
            DebugAreaDataGuard {
                operation: self.operation,
                operation_stats: &self.stats.area_data,
                identity: DebugAreaIdentity::new(area, addrspace, uaddr),
                start_us: arch::get_time_us(),
            }
        }
    }

    impl Drop for DebugOperationGuard {
        fn drop(&mut self) {
            self.stats.record(arch::get_time_us().saturating_sub(self.start_us));
        }
    }

    pub(super) struct DebugPhaseGuard<'a> {
        stats: &'a DebugPhaseStats,
        start_us: u64,
    }

    impl<'a> DebugPhaseGuard<'a> {
        fn new(stats: &'a DebugPhaseStats) -> Self {
            Self {
                stats,
                start_us: arch::get_time_us(),
            }
        }
    }

    impl Drop for DebugPhaseGuard<'_> {
        fn drop(&mut self) {
            self.stats.record(arch::get_time_us().saturating_sub(self.start_us));
        }
    }

    pub(super) struct DebugAreaDataGuard<'a> {
        operation: DebugOperation,
        operation_stats: &'a DebugPhaseStats,
        identity: DebugAreaIdentity,
        start_us: u64,
    }

    impl Drop for DebugAreaDataGuard<'_> {
        fn drop(&mut self) {
            let elapsed_us = arch::get_time_us().saturating_sub(self.start_us);
            self.operation_stats.record(elapsed_us);
            self.identity.kind.stats().operation(self.operation).record(elapsed_us);
            if elapsed_us >= DEBUG_SLOW_AREA_THRESHOLD_US {
                record_slow_area_event(self.operation, self.identity, elapsed_us);
            }
        }
    }

    struct DebugSlowAreaEvent {
        sequence: AtomicU64,
        operation: AtomicUsize,
        kind: AtomicUsize,
        addrspace: AtomicUsize,
        area: AtomicUsize,
        backing: AtomicUsize,
        ubase: AtomicUsize,
        uaddr: AtomicUsize,
        elapsed_us: AtomicU64,
    }

    impl DebugSlowAreaEvent {
        const fn new() -> Self {
            Self {
                sequence: AtomicU64::new(0),
                operation: AtomicUsize::new(0),
                kind: AtomicUsize::new(0),
                addrspace: AtomicUsize::new(0),
                area: AtomicUsize::new(0),
                backing: AtomicUsize::new(0),
                ubase: AtomicUsize::new(0),
                uaddr: AtomicUsize::new(0),
                elapsed_us: AtomicU64::new(0),
            }
        }

        fn write(&self, sequence: u64, operation: DebugOperation, identity: DebugAreaIdentity, elapsed_us: u64) {
            self.sequence.store(0, Ordering::Release);
            self.operation.store(operation as usize, Ordering::Relaxed);
            self.kind.store(identity.kind as usize, Ordering::Relaxed);
            self.addrspace.store(identity.addrspace, Ordering::Relaxed);
            self.area.store(identity.area, Ordering::Relaxed);
            self.backing.store(identity.backing, Ordering::Relaxed);
            self.ubase.store(identity.ubase, Ordering::Relaxed);
            self.uaddr.store(identity.uaddr, Ordering::Relaxed);
            self.elapsed_us.store(elapsed_us, Ordering::Relaxed);
            self.sequence.store(sequence, Ordering::Release);
        }

        fn snapshot(&self) -> Option<DebugSlowAreaEventSnapshot> {
            let sequence = self.sequence.load(Ordering::Acquire);
            if sequence == 0 {
                return None;
            }
            let snapshot = DebugSlowAreaEventSnapshot {
                sequence,
                operation: DebugOperation::from_raw(self.operation.load(Ordering::Relaxed)),
                kind: DebugAreaKind::from_raw(self.kind.load(Ordering::Relaxed)),
                addrspace: self.addrspace.load(Ordering::Relaxed),
                area: self.area.load(Ordering::Relaxed),
                backing: self.backing.load(Ordering::Relaxed),
                ubase: self.ubase.load(Ordering::Relaxed),
                uaddr: self.uaddr.load(Ordering::Relaxed),
                elapsed_us: self.elapsed_us.load(Ordering::Relaxed),
            };
            (self.sequence.load(Ordering::Acquire) == sequence).then_some(snapshot)
        }
    }

    struct DebugSlowAreaEventSnapshot {
        sequence: u64,
        operation: DebugOperation,
        kind: DebugAreaKind,
        addrspace: usize,
        area: usize,
        backing: usize,
        ubase: usize,
        uaddr: usize,
        elapsed_us: u64,
    }

    const DEBUG_SLOW_AREA_THRESHOLD_US: u64 = 10_000;
    const DEBUG_SLOW_AREA_EVENT_COUNT: usize = 32;

    static DEBUG_SLOW_AREA_NEXT: AtomicU64 = AtomicU64::new(0);
    static DEBUG_SLOW_AREA_EVENTS: [DebugSlowAreaEvent; DEBUG_SLOW_AREA_EVENT_COUNT] =
        [const { DebugSlowAreaEvent::new() }; DEBUG_SLOW_AREA_EVENT_COUNT];

    fn record_slow_area_event(operation: DebugOperation, identity: DebugAreaIdentity, elapsed_us: u64) {
        let sequence = DEBUG_SLOW_AREA_NEXT.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let index = (sequence as usize - 1) % DEBUG_SLOW_AREA_EVENT_COUNT;
        DEBUG_SLOW_AREA_EVENTS[index].write(sequence, operation, identity, elapsed_us);
    }

    static DEBUG_TRANSLATE_READ: DebugOperationStats = DebugOperationStats::new();
    static DEBUG_TRANSLATE_WRITE: DebugOperationStats = DebugOperationStats::new();
    static DEBUG_GET_FRAME: DebugOperationStats = DebugOperationStats::new();
    static DEBUG_MEMORY_FAULT: DebugOperationStats = DebugOperationStats::new();

    static DEBUG_AREA_UNKNOWN: DebugAreaKindStats = DebugAreaKindStats::new();
    static DEBUG_AREA_PRIVATE_ANONYMOUS: DebugAreaKindStats = DebugAreaKindStats::new();
    static DEBUG_AREA_SHARED_ANONYMOUS: DebugAreaKindStats = DebugAreaKindStats::new();
    static DEBUG_AREA_PRIVATE_FILE: DebugAreaKindStats = DebugAreaKindStats::new();
    static DEBUG_AREA_SHARED_FILE: DebugAreaKindStats = DebugAreaKindStats::new();
    static DEBUG_AREA_ELF: DebugAreaKindStats = DebugAreaKindStats::new();
    static DEBUG_AREA_STACK: DebugAreaKindStats = DebugAreaKindStats::new();
    static DEBUG_AREA_SHM: DebugAreaKindStats = DebugAreaKindStats::new();

    pub(crate) fn dump_lock_debug_stats() {
        println!("========== MapManager lock debug ==========");
        println!("  outer wait: scheduler blocked AddrSpace::map_manager");
        println!("  area-set: BTreeMap lookup, Area lock, and permission check");
        println!("  area-data: Area callback, including nested frame/page-cache waits");
        DEBUG_TRANSLATE_READ.dump("translate-read");
        DEBUG_TRANSLATE_WRITE.dump("translate-write");
        DEBUG_GET_FRAME.dump("get-frame");
        DEBUG_MEMORY_FAULT.dump("memory-fault");
        println!("  by area type:");
        DEBUG_AREA_UNKNOWN.dump(DebugAreaKind::Unknown.name());
        DEBUG_AREA_PRIVATE_ANONYMOUS.dump(DebugAreaKind::PrivateAnonymous.name());
        DEBUG_AREA_SHARED_ANONYMOUS.dump(DebugAreaKind::SharedAnonymous.name());
        DEBUG_AREA_PRIVATE_FILE.dump(DebugAreaKind::PrivateFile.name());
        DEBUG_AREA_SHARED_FILE.dump(DebugAreaKind::SharedFile.name());
        DEBUG_AREA_ELF.dump(DebugAreaKind::Elf.name());
        DEBUG_AREA_STACK.dump(DebugAreaKind::Stack.name());
        DEBUG_AREA_SHM.dump(DebugAreaKind::Shm.name());

        let mut slow_events = DEBUG_SLOW_AREA_EVENTS
            .iter()
            .filter_map(DebugSlowAreaEvent::snapshot)
            .collect::<Vec<_>>();
        slow_events.sort_by_key(|event| event.sequence);
        println!("  latest area-data calls >= {} us:", DEBUG_SLOW_AREA_THRESHOLD_US);
        for event in slow_events {
            println!(
                "    seq={} op={} type={} as={:#x} area={:#x} backing={:#x} ubase={:#x} uaddr={:#x} elapsed={} us",
                event.sequence,
                event.operation.name(),
                event.kind.name(),
                event.addrspace,
                event.area,
                event.backing,
                event.ubase,
                event.uaddr,
                event.elapsed_us,
            );
        }
        println!("===========================================");
    }
}

#[cfg(feature = "map-manager-lock-debug")]
pub(crate) use perf_debug::dump_lock_debug_stats;
#[cfg(feature = "map-manager-lock-debug")]
use perf_debug::{DebugOperation, DebugOperationGuard};
// PERF_DEBUG_END(map-manager-lock)

type LockedArea = SleepLockOnStack<Box<dyn Area>>;

/// The containing `AddrSpace` protects the Area set with
/// `SleepRwLockOnStack<Manager>`. Always acquire that outer lock before an
/// individual `LockedArea` is locked or removed from the set.
pub struct Manager {
    areas: BTreeMap<usize, LockedArea>,
    userstack_ubase: usize,
    userbrk: UserBrk,
    watchers: Vec<Arc<dyn MapManagerWatcher>>,
}

impl Manager {
    fn lock_area(area: Box<dyn Area>) -> LockedArea {
        SleepLockOnStack::new(area, "MapManager::area")
    }

    pub fn new() -> Self {
        Self {
            areas: BTreeMap::new(),
            userstack_ubase: 0,
            userbrk: UserBrk::new(),
            watchers: Vec::new(),
        }
    }

    pub fn fork(&mut self, self_pagetable: &SpinLock<PageTable>, addrspace: &Arc<AddrSpace>) -> Manager {
        let watchers = self.watchers.as_slice();
        let mut tlb_cpu_mask = 0;

        let new_areas = self
            .areas
            .iter()
            .map(|(ubase, area)| {
                let mut area = area.lock();
                Self::notify_before_map_change_to(
                    watchers,
                    MapChange {
                        uaddr: area.ubase(),
                        page_count: area.page_count(),
                        event: MapChangeEvent::Remap,
                    },
                );
                (
                    *ubase,
                    Self::lock_area(area.fork(self_pagetable, &mut tlb_cpu_mask, addrspace)),
                )
            })
            .collect();
        arch::flush_tlb_cpu_mask(tlb_cpu_mask);

        Self {
            areas: new_areas,
            userstack_ubase: self.userstack_ubase,
            userbrk: self.userbrk.clone(),
            watchers: Vec::new(),
        }
    }

    pub fn add_watcher(&mut self, watcher: Arc<dyn MapManagerWatcher>) {
        if self.watchers.iter().any(|registered| Arc::ptr_eq(registered, &watcher)) {
            return;
        }
        self.watchers.push(watcher);
    }

    pub fn remove_watcher(&mut self, watcher: &Arc<dyn MapManagerWatcher>) {
        self.watchers.retain(|registered| !Arc::ptr_eq(registered, watcher));
    }

    fn notify_before_map_change_to(watchers: &[Arc<dyn MapManagerWatcher>], change: MapChange) {
        MapChangeNotifier::new(watchers).before_map_change(change);
    }

    fn notify_before_map_change(&self, change: MapChange) {
        Self::notify_before_map_change_to(&self.watchers, change);
    }

    /// Find a suitable virtual address for mmap allocation
    ///
    /// Searches for a contiguous region of virtual memory that can accommodate
    /// the requested number of pages, starting from USER_MAP_BASE.
    ///
    /// # Arguments
    /// * `page_count` - Number of pages to allocate
    ///
    /// # Returns
    /// * `Some(usize)` - Base address for the allocation if found
    /// * `None` - If no suitable address space is available
    pub fn find_mmap_ubase(&self, page_count: usize) -> Option<usize> {
        if page_count == 0 {
            return None;
        }

        let required_size = page_count * arch::PGSIZE;
        let mut candidate_addr = config::USER_MAP_BASE;

        // Ensure candidate address is page-aligned
        candidate_addr = (candidate_addr + arch::PGSIZE - 1) & !(arch::PGSIZE - 1);

        // Iterate through existing areas in ascending order to find a gap
        for (&area_base, area) in &self.areas {
            let area = area.lock();
            let area_end = area_base + area.size();

            // Check if candidate address is before this area and has enough space
            if candidate_addr + required_size <= area_base {
                // Found a suitable gap before this area
                // ktrace!("Found mmap address {:#x} for {} pages (gap before area at {:#x})",
                //        candidate_addr, page_count, area_base);
                return Some(candidate_addr);
            }

            // If candidate overlaps with or is too close to this area,
            // move to after this area
            if candidate_addr < area_end {
                candidate_addr = area_end;
                // Re-align to page boundary
                candidate_addr = (candidate_addr + arch::PGSIZE - 1) & !(arch::PGSIZE - 1);
            }
        }

        // Check if there's space after all existing areas
        // We need to ensure we don't exceed reasonable address space limits
        // For safety, let's limit to addresses below the user stack region
        let max_mmap_addr = if self.userstack_ubase > 0 {
            self.userstack_ubase
        } else {
            config::USER_STACK_TOP - config::USER_STACK_PAGE_COUNT_MAX * arch::PGSIZE
        };

        if candidate_addr + required_size <= max_mmap_addr {
            ktrace!(
                "Found mmap address {:#x} for {} pages (after all areas)",
                candidate_addr,
                page_count
            );
            Some(candidate_addr)
        } else {
            ktrace!(
                "No suitable mmap address found for {} pages (would exceed limit {:#x})",
                page_count,
                max_mmap_addr
            );
            None
        }
    }

    fn overlap_scan_start(&self, start: usize) -> usize {
        self.areas.range(..=start).next_back().map(|(k, _)| *k).unwrap_or(start)
    }

    pub fn is_map_range_overlapped(&self, start: usize, page_count: usize) -> bool {
        let end = start + page_count * arch::PGSIZE;

        for (&area_base, area) in self.areas.range(self.overlap_scan_start(start)..) {
            if end <= area_base {
                break;
            }

            let area = area.lock();
            if area_base + area.size() <= start {
                continue;
            }

            return true;
        }

        return false;
    }

    pub fn map_area(&mut self, uaddr: usize, area: Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(
            !self.is_map_range_overlapped(uaddr, area.page_count()),
            "Address range is not free"
        );
        self.areas.insert(uaddr, Self::lock_area(area));
    }

    pub fn snapshot(&self) -> Vec<MapAreaInfo> {
        self.areas
            .iter()
            .map(|(&start, area)| {
                let area = area.lock();
                let mut info = area.map_area_info();
                info.start = start;
                info.end = start + area.size();
                info.perm = area.perm();

                if info.path.is_none() && self.is_userbrk_area(info.start, info.end) {
                    info.path = Some("[heap]".into());
                }

                info
            })
            .collect()
    }

    fn is_userbrk_area(&self, start: usize, end: usize) -> bool {
        let heap_end = config::USER_BRK_BASE + self.userbrk.page_count * arch::PGSIZE;
        start >= config::USER_BRK_BASE && end <= heap_end
    }

    fn find_overlapped_areas(&self, start: usize, end: usize) -> Vec<usize> {
        let mut overlapped_areas = Vec::new();

        for (&area_base, area) in self.areas.range(self.overlap_scan_start(start)..) {
            if end <= area_base {
                break;
            }

            let area = area.lock();
            if area_base + area.size() <= start {
                continue;
            }

            overlapped_areas.push(area_base);
        }

        overlapped_areas
    }

    /// Map an area at a fixed address, handling any overlapping areas
    ///
    /// This function maps a new area at the specified address, automatically handling
    /// any existing areas that overlap with the new area's range. Overlapping areas
    /// will be split and/or removed as necessary.
    ///
    /// # Arguments
    /// * `uaddr` - Base address where the area should be mapped (must be page-aligned)
    /// * `area` - The area to be mapped
    ///
    /// # Returns
    /// * `Ok(())` - Success
    /// * `Err(Errno)` - Error during mapping or area splitting
    ///
    /// # Behavior
    /// - If existing areas completely overlap with the new area, they are removed
    /// - If existing areas partially overlap, they are split to preserve non-overlapping parts
    /// - The new area is then inserted at the specified address
    ///
    /// # Examples
    /// ```
    /// // Map a new area that may overlap with existing mappings
    /// let new_area = Box::new(AnonymousArea::new(addr, perm, page_count));
    /// manager.map_area_fixed(addr, new_area)?;
    /// ```
    pub fn map_area_fixed(&mut self, uaddr: usize, area: Box<dyn Area>, pagetable: &SpinLock<PageTable>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");

        let new_area_end = uaddr + area.size();

        for overlapping_base in self.find_overlapped_areas(uaddr, new_area_end) {
            let mut middle = self.areas.remove(&overlapping_base).unwrap().into_inner();
            let overlapping_end = overlapping_base + middle.size();

            // KEEP Left part [overlapping_base, uaddr)
            if overlapping_base < uaddr {
                let left;
                (left, middle) = middle.split(uaddr);
                self.areas.insert(overlapping_base, Self::lock_area(left));
            }

            // KEEP Right part [new_area_end, overlapping_end)
            if new_area_end < overlapping_end {
                let right;
                (middle, right) = middle.split(new_area_end);
                self.areas.insert(new_area_end, Self::lock_area(right));
            }

            self.notify_before_map_change(MapChange {
                uaddr: middle.ubase(),
                page_count: middle.page_count(),
                event: MapChangeEvent::Remap,
            });
            // UNMAP Middle part [max(overlapping_base, uaddr), min(overlapping_end, new_area_end))
            middle.unmap(pagetable);
        }

        // Now we can safely insert the new area
        self.areas.insert(uaddr, Self::lock_area(area));
    }

    pub fn unmap_area(&mut self, uaddr: usize, page_count: usize, pagetable: &SpinLock<PageTable>) -> SysResult<()> {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(page_count > 0, "page_count should be greater than 0");

        let uaddr_end = uaddr + page_count * arch::PGSIZE;

        // Process each intersecting area
        for area_base in self.find_overlapped_areas(uaddr, uaddr_end) {
            // Remove the area from the map
            let mut middle = self.areas.remove(&area_base).unwrap().into_inner();
            let area_end = area_base + middle.size();

            // KEEP Left part [area_base, uaddr)
            if area_base < uaddr {
                let left;
                (left, middle) = middle.split(uaddr);
                self.areas.insert(area_base, Self::lock_area(left));
            }

            // KEEP Right part [uaddr_end, area_end)
            if uaddr_end < area_end {
                let right;
                (middle, right) = middle.split(uaddr_end);
                self.areas.insert(uaddr_end, Self::lock_area(right));
            }

            self.notify_before_map_change(MapChange {
                uaddr: middle.ubase(),
                page_count: middle.page_count(),
                event: MapChangeEvent::Unmap,
            });
            // UNMAP Middle part [max(area_base, uaddr), min(area_end, uaddr_end))
            middle.unmap(pagetable);
        }

        Ok(())
    }

    /// Set permissions for a specific range of pages
    ///
    /// Changes the permissions for pages in the range [uaddr, uaddr + page_count * PGSIZE).
    /// If the range spans part of an existing area, the area will be split to accommodate
    /// the permission change.
    ///
    /// # Arguments
    /// * `uaddr` - Start address (must be page-aligned)
    /// * `page_count` - Number of pages to modify
    /// * `perm` - New permissions to apply
    /// * `pagetable` - Page table to update
    ///
    /// # Returns
    /// * `Ok(())` - Success
    /// * `Err(Errno)` - Error (e.g., invalid address range, no mapping found)
    pub fn set_map_area_perm(
        &mut self,
        uaddr: usize,
        page_count: usize,
        perm: MapPerm,
        pagetable: &SpinLock<PageTable>,
    ) -> SysResult<()> {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr must be page-aligned");

        if page_count == 0 {
            return Ok(());
        }

        let uaddr_end = uaddr + page_count * arch::PGSIZE;
        let overlapped_areas = self.find_overlapped_areas(uaddr, uaddr_end);

        for overlapped_base in overlapped_areas.iter() {
            self.areas.get(overlapped_base).unwrap().lock().check_set_perm(perm)?;
        }

        let mut tlb_cpu_mask = 0;
        for overlapped_base in overlapped_areas {
            let mut middle = self.areas.remove(&overlapped_base).unwrap().into_inner();
            let overlapped_end = overlapped_base + middle.size();

            if overlapped_base < uaddr {
                let left;
                (left, middle) = middle.split(uaddr);
                self.areas.insert(overlapped_base, Self::lock_area(left));
            }

            if uaddr_end < overlapped_end {
                let right;
                (middle, right) = middle.split(uaddr_end);
                self.areas.insert(uaddr_end, Self::lock_area(right));
            }

            self.notify_before_map_change(MapChange {
                uaddr: middle.ubase(),
                page_count: middle.page_count(),
                event: MapChangeEvent::PermChange(perm),
            });
            middle.set_perm(perm, pagetable, &mut tlb_cpu_mask);
            self.areas.insert(middle.ubase(), Self::lock_area(middle));
        }
        arch::flush_tlb_cpu_mask(tlb_cpu_mask);

        Ok(())
    }

    pub fn create_user_stack(
        &mut self,
        argv: &[&str],
        envp: &[&str],
        auxv: &Auxv,
        addrspace: &Arc<AddrSpace>,
    ) -> Result<usize, Errno> {
        assert!(self.userstack_ubase == 0, "User stack already created");

        let mut userstack = Box::new(UserStack::new());
        let ubase = config::USER_STACK_TOP - config::USER_STACK_PAGE_COUNT_MAX * arch::PGSIZE;

        userstack.bind_addrspace(addrspace);

        let top = userstack.push_argv_envp_auxv(argv, envp, auxv, addrspace)?;

        self.map_area(ubase, userstack as Box<dyn Area>);
        self.userstack_ubase = ubase;

        Ok(top)
    }

    pub fn translate_read(&self, uaddr: usize, addrspace: &AddrSpace) -> Option<PinPageFrame> {
        // PERF_DEBUG(map-manager-lock): Attribute lookup versus Area callback time.
        #[cfg(feature = "map-manager-lock-debug")]
        let debug = DebugOperationGuard::new(DebugOperation::TranslateRead);
        let mut area = {
            #[cfg(feature = "map-manager-lock-debug")]
            let _phase = debug.area_set();
            let (_, area) = self.areas.range(..=uaddr).next_back()?;
            let area = area.lock();
            area.perm().contains(MapPerm::R).then_some(area)?
        };

        #[cfg(feature = "map-manager-lock-debug")]
        let _phase = debug.area_data(area.as_mut(), addrspace, uaddr);
        area.translate_read(uaddr, addrspace)
    }

    pub fn translate_write(&self, uaddr: usize, addrspace: &AddrSpace) -> Option<PinPageFrame> {
        // PERF_DEBUG(map-manager-lock): Attribute lookup versus Area callback time.
        #[cfg(feature = "map-manager-lock-debug")]
        let debug = DebugOperationGuard::new(DebugOperation::TranslateWrite);
        let watchers = self.watchers.as_slice();
        let mut area = {
            #[cfg(feature = "map-manager-lock-debug")]
            let _phase = debug.area_set();
            let (_, area) = self.areas.range(..=uaddr).next_back()?;
            let area = area.lock();
            area.perm().contains(MapPerm::W).then_some(area)?
        };

        let map_change_notifier = MapChangeNotifier::new(watchers);
        #[cfg(feature = "map-manager-lock-debug")]
        let _phase = debug.area_data(area.as_mut(), addrspace, uaddr);
        area.translate_write(uaddr, addrspace, &map_change_notifier)
    }

    pub fn get_frame(&self, uaddr: usize, addrspace: &AddrSpace) -> Option<PinPageFrame> {
        // PERF_DEBUG(map-manager-lock): Attribute lookup versus Area callback time.
        #[cfg(feature = "map-manager-lock-debug")]
        let debug = DebugOperationGuard::new(DebugOperation::GetFrame);
        let watchers = self.watchers.as_slice();
        let mut area = {
            #[cfg(feature = "map-manager-lock-debug")]
            let _phase = debug.area_set();
            let (_, area) = self.areas.range(..=uaddr).next_back()?;
            let area = area.lock();
            area.perm().contains(MapPerm::W).then_some(area)?
        };

        let map_change_notifier = MapChangeNotifier::new(watchers);
        #[cfg(feature = "map-manager-lock-debug")]
        let _phase = debug.area_data(area.as_mut(), addrspace, uaddr);
        area.get_frame(uaddr, addrspace, &map_change_notifier)
    }

    pub fn try_to_fix_memory_fault(
        &self,
        uaddr: usize,
        access_type: MemAccessType,
        addrspace: &AddrSpace,
    ) -> Result<(), MemoryFaultError> {
        // PERF_DEBUG(map-manager-lock): Attribute lookup versus Area callback time.
        #[cfg(feature = "map-manager-lock-debug")]
        let debug = DebugOperationGuard::new(DebugOperation::MemoryFault);
        let watchers = self.watchers.as_slice();
        let mut area = {
            #[cfg(feature = "map-manager-lock-debug")]
            let _phase = debug.area_set();
            let Some((_ubase, area)) = self.areas.range(..=uaddr).next_back() else {
                return Err(MemoryFaultError::SegvMapError);
            };
            let area = area.lock();
            if uaddr < area.ubase() || uaddr - area.ubase() >= area.size() {
                return Err(MemoryFaultError::SegvMapError);
            }
            if !access_type.match_perm(area.perm()) {
                return Err(MemoryFaultError::SegvAccessError);
            }
            area
        };

        let map_change_notifier = MapChangeNotifier::new(watchers);
        #[cfg(feature = "map-manager-lock-debug")]
        let _phase = debug.area_data(area.as_mut(), addrspace, uaddr);
        match area.try_to_fix_memory_fault(uaddr, access_type, addrspace, &map_change_notifier) {
            Ok(()) => Ok(()),
            Err(signal) => {
                // crate::kinfo!(
                //     "Area {} at {:#x} failed to fix memory fault at {:#x} for access type {:?}",
                //     area.type_name(),
                //     area.ubase(),
                //     uaddr,
                //     access_type
                // );
                Err(signal)
            }
        }
    }

    pub fn increase_userbrk(
        &mut self,
        new_ubrk: usize,
        pagetable: &SpinLock<PageTable>,
        addrspace: &Arc<AddrSpace>,
    ) -> Result<usize, Errno> {
        if new_ubrk == 0 {
            return Ok(self.userbrk.ubrk);
        }

        if new_ubrk < config::USER_BRK_BASE {
            return Ok(self.userbrk.ubrk);
        }

        if new_ubrk > config::USER_BRK_MAX {
            return Err(Errno::ENOMEM);
        }

        let new_page_count = if new_ubrk == config::USER_BRK_BASE {
            0
        } else {
            (new_ubrk - config::USER_BRK_BASE + arch::PGSIZE - 1) / arch::PGSIZE
        };

        if new_page_count > self.userbrk.page_count {
            // Growing: map new pages
            let ubase = config::USER_BRK_BASE + self.userbrk.page_count * arch::PGSIZE;
            let mut new_area = Box::new(PrivateAnonymousArea::new(
                ubase,
                MapPerm::R | MapPerm::W | MapPerm::U,
                new_page_count - self.userbrk.page_count,
            ));

            new_area.bind_addrspace(addrspace);

            self.map_area(ubase, new_area);
        } else if new_page_count < self.userbrk.page_count {
            // Shrinking: unmap released pages
            let unmap_base = config::USER_BRK_BASE + new_page_count * arch::PGSIZE;
            let unmap_count = self.userbrk.page_count - new_page_count;
            let _ = self.unmap_area(unmap_base, unmap_count, pagetable);
        }

        self.userbrk.page_count = new_page_count;
        self.userbrk.ubrk = new_ubrk;

        Ok(self.userbrk.ubrk)
    }

    /// Print all mapped areas for debugging purposes
    ///
    /// Outputs information about each mapped area including:
    /// - Base address and size
    /// - Area type
    /// - Address range
    /// Print information about all mapped areas for debugging
    ///
    /// This function outputs details about all currently mapped areas to help
    /// with debugging memory management issues.
    #[allow(dead_code)]
    pub fn print_all_areas(&self) {
        print!("=== Memory Area Manager Status ===\n");
        print!("Total areas: {}\n", self.areas.len());
        print!("User stack base: {:#x}\n", self.userstack_ubase);
        print!(
            "User brk: {:#x} (page count: {})\n",
            self.userbrk.ubrk, self.userbrk.page_count
        );
        print!("\n");

        if self.areas.is_empty() {
            print!("No mapped areas\n");
        } else {
            print!("Mapped areas:\n");
            for (index, (&base_addr, area)) in self.areas.iter().enumerate() {
                let area = area.lock();
                let end_addr = base_addr + area.size();
                print!(
                    "  {}. {} [{:#x}, {:#x}) {:?} - {}  bytes\n",
                    index + 1,
                    area.type_name(),
                    base_addr,
                    end_addr,
                    area.perm(),
                    area.size()
                );
            }
        }
        print!("=== End Memory Area Status ===\n");
    }

    pub fn cleanup(&mut self, pagetable: &SpinLock<PageTable>) {
        let areas = core::mem::take(&mut self.areas);
        for (_, area) in areas {
            let mut area = area.into_inner();
            area.unmap(pagetable);
        }
    }
}
