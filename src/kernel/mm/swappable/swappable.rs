use alloc::sync::Arc;

use crate::arch;
use crate::kernel::mm::PhysPageFrame;
use crate::klib::intrusive_lru::{IntrusiveLruNode, IntrusiveLruNodeOps};
use crate::klib::{BucketSleepLock, BucketSleepLockGuard, SleepLock};

pub struct TlbInvalidationToken;

/// Serializes TLB invalidation batches that can touch pages from one mapping family.
///
/// The lock is required because a batch rescans shared page storage after the
/// flush instead of retaining an owner list. Without serialization, batch A can
/// skip an empty slot, batch B can materialize and pin a page in that slot, and
/// then A's rescan can consume B's unowned `tlb_invalidation_pins` entry before
/// B flushes its TLB. Keeping one lock per family prevents that race while still
/// allowing unrelated mapping families to invalidate concurrently.
pub struct TlbInvalidationLock(SleepLock<()>);

impl TlbInvalidationLock {
    pub const fn new(name: &'static str) -> Self {
        Self(SleepLock::new((), name))
    }

    pub fn with_batch<R>(&self, f: impl FnOnce(&TlbInvalidationToken) -> R) -> R {
        let _guard = self.0.lock();
        f(&TlbInvalidationToken)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapError {
    #[cfg(feature = "swap-memory")]
    NoSpace,
    Io,
    InvalidBacking,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccessDirty {
    pub accessed: bool,
    pub dirty: bool,
}

impl From<(bool, bool)> for AccessDirty {
    fn from((accessed, dirty): (bool, bool)) -> Self {
        Self { accessed, dirty }
    }
}

pub trait SwappableBackendOps: Clone + Send + Sync + Sized + 'static {
    type SwappedOutContext;

    fn inspect_access_dirty_no_flush(&self, frame: &PhysPageFrame) -> (Self::SwappedOutContext, AccessDirty, bool);

    fn inspect_access_dirty(&self, frame: &PhysPageFrame) -> (Self::SwappedOutContext, AccessDirty) {
        let (context, access_dirty, tlb_changed) = self.inspect_access_dirty_no_flush(frame);
        if tlb_changed {
            arch::flush_tlb_all();
        }
        (context, access_dirty)
    }

    /// Clears every matching PTE without flushing the TLB.
    ///
    /// `Some` means at least one PTE changed, so the caller must complete a
    /// TLB invalidation before writing out or releasing the physical frame.
    fn unmap_no_flush(&self, context: &Self::SwappedOutContext, frame: &PhysPageFrame) -> Option<AccessDirty>;

    fn read_in(&self) -> Result<PhysPageFrame, SwapError>;

    fn write_out(&mut self, frame: &PhysPageFrame, dirty: bool, mapping_refs: usize) -> Result<(), SwapError>;

    fn release(&mut self);

    fn is_swappable(&self) -> bool {
        true
    }

    /// Whether the backend wants to retain unmapped pages in memory.
    /// If this returns `true`, the page will not be removed from the LRU
    fn retain_unmapped_pages(&self) -> bool {
        false
    }

    /// When the `mapping_refs` of a swappable frame reaches zero,
    /// this function is called to notify the backend that
    /// the page is no longer mapped in any address space.
    fn remove_unmapped_page(&self, _page: &Arc<SwappableFrame<Self>>) {}
}

pub(crate) trait ResidentPageGuard {
    fn frame(&self) -> &PhysPageFrame;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapOutResult {
    /// The frame was successfully evicted from memory.
    Evicted,

    /// The frame is still in use and cannot be swapped out at this time.
    Referenced,

    /// The frame can be released directly
    /// without writing it back to swap space
    Released,

    /// The frame could not be reclaimed.
    Failed,

    /// The frame is busy but no longer eligible for the reclaim LRU.
    Busy,
}

pub trait SwappableFrameOps: Send + Sync {
    fn lru_node(&self) -> &IntrusiveLruNode<dyn SwappableFrameOps>;

    fn reclaim_class(&self) -> ReclaimClass;

    fn try_swap_out(self: Arc<Self>) -> Result<SwapOutResult, SwapError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReclaimClass {
    CleanUnmapped,
    DirtyUnmapped,
    Mapped,
}

impl ReclaimClass {
    pub const COUNT: usize = 3;

    pub const fn index(self) -> usize {
        self as usize
    }
}

impl IntrusiveLruNodeOps<dyn SwappableFrameOps> for dyn SwappableFrameOps {
    fn lru_node(&self) -> &IntrusiveLruNode<dyn SwappableFrameOps> {
        SwappableFrameOps::lru_node(self)
    }
}

enum SwappableFrameState {
    /// The page is resident in memory and has a physical frame.
    In {
        frame: PhysPageFrame,
        dirty: bool,
        referenced: bool,
    },

    /// The page is not resident in memory and can be read from its backend.
    Out,

    /// The page is deleted but still pinned by some code,
    /// and the physical frame is still valid and can be used.
    Invalidating { _frame: PhysPageFrame },

    /// The page is deleted and no longer pinned by any code.
    Invalid,
}

struct SwappableFrameInner<B> {
    state: SwappableFrameState,
    backend: B,
    mapping_refs: usize,
    pins: usize,
    tlb_invalidation_pins: usize,
}

pub struct SwappableFrame<B: SwappableBackendOps> {
    inner: BucketSleepLock<SwappableFrameInner<B>>,
    lru_node: IntrusiveLruNode<dyn SwappableFrameOps>,
}

impl<B: SwappableBackendOps> SwappableFrame<B> {
    pub fn new_mapped(backend: B, frame: PhysPageFrame) -> Arc<Self> {
        Self::new_with_mapping_refs(
            backend,
            SwappableFrameState::In {
                frame,
                dirty: false,
                referenced: true,
            },
            1,
        )
    }

    pub(super) fn new_out(backend: B, mapping_refs: usize) -> Arc<Self> {
        Self::new_with_mapping_refs(backend, SwappableFrameState::Out, mapping_refs)
    }

    fn new_with_mapping_refs(backend: B, state: SwappableFrameState, mapping_refs: usize) -> Arc<Self> {
        let link_to_lru = backend.is_swappable() && matches!(state, SwappableFrameState::In { .. });
        let page = Arc::new(Self {
            inner: BucketSleepLock::new(
                SwappableFrameInner {
                    state,
                    backend,
                    mapping_refs,
                    pins: 0,
                    tlb_invalidation_pins: 0,
                },
                "SwappableFrame::inner",
            ),
            lru_node: IntrusiveLruNode::new(),
        });

        if link_to_lru {
            let page_ops: Arc<dyn SwappableFrameOps> = page.clone();
            super::swapper::link(page_ops);
        }
        page
    }

    fn ensure_resident_locked_with<E>(
        self: &Arc<Self>,
        inner: &mut SwappableFrameInner<B>,
        read_in: impl FnOnce(&B) -> Result<Option<PhysPageFrame>, E>,
    ) -> Result<bool, E> {
        if inner.mapping_refs == 0 && !inner.backend.retain_unmapped_pages() {
            return Ok(false);
        }

        if matches!(&inner.state, SwappableFrameState::Out) {
            let Some(frame) = read_in(&inner.backend)? else {
                inner.state = SwappableFrameState::Invalid;
                return Ok(false);
            };
            inner.state = SwappableFrameState::In {
                frame,
                dirty: false,
                referenced: true,
            };
            if inner.backend.is_swappable() {
                self.relink_locked(inner);
            }
        }
        Ok(!matches!(
            inner.state,
            SwappableFrameState::Invalidating { .. } | SwappableFrameState::Invalid
        ))
    }

    fn ensure_resident_locked(self: &Arc<Self>, inner: &mut SwappableFrameInner<B>) -> Result<(), SwapError> {
        let resident = self.ensure_resident_locked_with(inner, |backend| match backend.read_in() {
            Ok(frame) => Ok(Some(frame)),
            Err(SwapError::InvalidBacking) => Ok(None),
            Err(err) => Err(err),
        })?;
        resident.then_some(()).ok_or(SwapError::InvalidBacking)
    }

    pub fn ensure_page<'a>(self: &'a Arc<Self>) -> Result<SwappableFrameGuard<'a, B>, SwapError> {
        self.ensure_page_with(|backend| match backend.read_in() {
            Ok(frame) => Ok(Some(frame)),
            Err(SwapError::InvalidBacking) => Ok(None),
            Err(err) => Err(err),
        })?
        .ok_or(SwapError::InvalidBacking)
    }

    /// Returns the resident page without reading an `Out` page back in.
    pub fn try_get_page(self: &Arc<Self>) -> Option<SwappableFrameGuard<'_, B>> {
        let inner = self.inner.lock();
        if !matches!(inner.state, SwappableFrameState::In { .. }) {
            return None;
        }
        Some(SwappableFrameGuard { owner: self, inner })
    }

    /// Loads an `Out` page while holding its inner lock.
    ///
    /// `Ok(None)` means that the backing is permanently invalid, so the page
    /// transitions to `Invalid` before any waiter can retry the load.
    pub(super) fn ensure_page_with<'a, E>(
        self: &'a Arc<Self>,
        read_in: impl FnOnce(&B) -> Result<Option<PhysPageFrame>, E>,
    ) -> Result<Option<SwappableFrameGuard<'a, B>>, E> {
        let mut inner = self.inner.lock();
        if !self.ensure_resident_locked_with(&mut inner, read_in)? {
            return Ok(None);
        }
        Ok(Some(SwappableFrameGuard { owner: self, inner }))
    }

    pub fn pin_page(self: &Arc<Self>, write: bool) -> Result<SwappableFramePin<B>, SwapError> {
        let mut inner = self.inner.lock();
        self.ensure_resident_locked(&mut inner)?;
        let kpage = match &mut inner.state {
            SwappableFrameState::In { frame, dirty, .. } => {
                *dirty |= write;
                frame.get_page()
            }
            SwappableFrameState::Out | SwappableFrameState::Invalidating { .. } | SwappableFrameState::Invalid => {
                unreachable!()
            }
        };
        inner.pins = inner.pins.checked_add(1).expect("swappable frame pin count overflow");
        if inner.backend.is_swappable() {
            self.relink_locked(&inner);
        }
        drop(inner);
        Ok(SwappableFramePin {
            owner: self.clone(),
            kpage,
        })
    }

    pub fn lru_node(&self) -> &IntrusiveLruNode<dyn SwappableFrameOps> {
        &self.lru_node
    }

    pub fn mapping_refs(&self) -> usize {
        self.inner.lock().mapping_refs
    }

    /// Collects PTE A/D bits from mapped copies and returns the merged dirty state.
    pub fn collect_mapped_access_dirty(self: &Arc<Self>) -> bool {
        let (dirty, tlb_changed) = self.collect_mapped_access_dirty_no_flush();
        if tlb_changed {
            arch::flush_tlb_all();
        }
        dirty
    }

    /// Collects PTE A/D bits without flushing so a higher-level scan can batch
    /// all of its page-table updates into one TLB invalidation.
    pub fn collect_mapped_access_dirty_no_flush(self: &Arc<Self>) -> (bool, bool) {
        let mut inner = self.inner.lock();
        let (access_dirty, tlb_changed) = {
            let inner = &*inner;
            let SwappableFrameState::In { frame, .. } = &inner.state else {
                return (false, false);
            };
            let (_, access_dirty, tlb_changed) = inner.backend.inspect_access_dirty_no_flush(frame);
            (access_dirty, tlb_changed)
        };

        let is_swappable = inner.backend.is_swappable();
        let SwappableFrameState::In { dirty, referenced, .. } = &mut inner.state else {
            unreachable!();
        };
        *dirty |= access_dirty.dirty;
        *referenced |= access_dirty.accessed;
        let dirty = *dirty;
        if is_swappable && (access_dirty.accessed || access_dirty.dirty) {
            self.relink_locked(&inner);
        }
        (dirty, tlb_changed)
    }

    pub fn clear_dirty(self: &Arc<Self>) {
        let mut inner = self.inner.lock();
        if let SwappableFrameState::In { dirty, .. } = &mut inner.state {
            *dirty = false;
        }
        if inner.backend.is_swappable() {
            self.relink_locked(&inner);
        }
    }

    pub fn is_invalid(&self) -> bool {
        matches!(
            self.inner.lock().state,
            SwappableFrameState::Invalidating { .. } | SwappableFrameState::Invalid
        )
    }

    pub fn can_remove_unmapped_out(&self) -> bool {
        let inner = self.inner.lock();
        inner.mapping_refs == 0 && matches!(inner.state, SwappableFrameState::Out)
    }

    pub(super) fn request_remove_unmapped_page(self: &Arc<Self>) {
        let backend = {
            let inner = self.inner.lock();
            (inner.mapping_refs == 0 && matches!(inner.state, SwappableFrameState::Out)).then(|| inner.backend.clone())
        };
        if let Some(backend) = backend {
            backend.remove_unmapped_page(self);
        }
    }

    pub fn invalidate(self: &Arc<Self>) {
        let mut inner = self.inner.lock();
        if matches!(
            inner.state,
            SwappableFrameState::Invalidating { .. } | SwappableFrameState::Invalid
        ) {
            return;
        }

        if inner.backend.is_swappable() {
            let page: Arc<dyn SwappableFrameOps> = self.clone();
            super::swapper::unlink(&page);
        }

        if inner.mapping_refs != 0
            && let SwappableFrameState::In { frame, .. } = &inner.state
        {
            let (context, _) = inner.backend.inspect_access_dirty(frame);
            if inner.backend.unmap_no_flush(&context, frame).is_some() {
                arch::flush_tlb_all();
            }
        }

        let state = core::mem::replace(&mut inner.state, SwappableFrameState::Invalid);
        if let SwappableFrameState::In { frame, .. } = state
            && inner.pins != 0
        {
            inner.state = SwappableFrameState::Invalidating { _frame: frame };
        }
    }

    /// Runs `f` on a resident frame and records its access/dirty state.
    /// `accessed` reports a direct kernel access not represented by the returned PTE state.
    pub fn with_resident_and_record_ad<R>(
        self: &Arc<Self>,
        accessed: bool,
        f: impl FnOnce(&PhysPageFrame) -> (R, AccessDirty),
    ) -> Option<R> {
        let mut inner = self.inner.lock();
        let SwappableFrameState::In {
            frame,
            dirty,
            referenced,
        } = &mut inner.state
        else {
            return None;
        };
        let (result, access_dirty) = f(frame);
        let accessed = accessed || access_dirty.accessed;
        *referenced |= accessed;
        *dirty |= access_dirty.dirty;
        if inner.backend.is_swappable() && (accessed || access_dirty.dirty) {
            self.relink_locked(&inner);
        }
        Some(result)
    }

    /// Begins a TLB invalidation for this swappable frame.
    pub fn begin_tlb_invalidation(
        self: &Arc<Self>,
        _token: &TlbInvalidationToken,
        unmap: impl FnOnce(&PhysPageFrame) -> Option<AccessDirty>,
    ) {
        let mut inner = self.inner.lock();
        inner.pins = inner.pins.checked_add(1).expect("swappable frame pin count overflow");
        inner.tlb_invalidation_pins = inner
            .tlb_invalidation_pins
            .checked_add(1)
            .expect("swappable frame TLB invalidation pin count overflow");

        let access_dirty = match &inner.state {
            SwappableFrameState::In { frame, .. } => unmap(frame),
            SwappableFrameState::Out | SwappableFrameState::Invalidating { .. } | SwappableFrameState::Invalid => None,
        };
        if let Some(access_dirty) = access_dirty {
            let is_swappable = inner.backend.is_swappable();
            let SwappableFrameState::In { dirty, referenced, .. } = &mut inner.state else {
                unreachable!();
            };
            *dirty |= access_dirty.dirty;
            *referenced |= access_dirty.accessed;
            if is_swappable && (access_dirty.accessed || access_dirty.dirty) {
                self.relink_locked(&inner);
            }
        }
    }

    pub fn finish_tlb_invalidation(self: &Arc<Self>, _token: &TlbInvalidationToken) -> bool {
        let mut inner = self.inner.lock();
        if inner.tlb_invalidation_pins == 0 {
            return false;
        }
        inner.tlb_invalidation_pins -= 1;
        assert!(inner.pins > 0, "swappable frame pin count underflow");
        inner.pins -= 1;
        if inner.pins == 0 && matches!(inner.state, SwappableFrameState::Invalidating { .. }) {
            inner.state = SwappableFrameState::Invalid;
        }
        true
    }

    pub fn add_mapping_ref(self: &Arc<Self>) {
        let mut inner = self.inner.lock();
        inner.mapping_refs = inner
            .mapping_refs
            .checked_add(1)
            .expect("swappable frame mapping reference count overflow");
        if inner.backend.is_swappable() {
            self.relink_locked(&inner);
        }
    }

    pub fn release_mapping_ref(self: &Arc<Self>) -> usize {
        let mut inner = self.inner.lock();
        let mapping_refs = self.release_mapping_ref_locked(&mut inner);
        let remove_backend =
            (mapping_refs == 0 && matches!(inner.state, SwappableFrameState::Out)).then(|| inner.backend.clone());
        drop(inner);
        if let Some(backend) = remove_backend {
            backend.remove_unmapped_page(self);
        }
        mapping_refs
    }

    pub fn try_swap_out(self: Arc<Self>) -> Result<SwapOutResult, SwapError> {
        let mut inner = self.inner.lock();

        if inner.pins != 0 {
            // The frame is pinned by other code.
            return Ok(if inner.mapping_refs != 0 || inner.backend.retain_unmapped_pages() {
                SwapOutResult::Referenced
            } else {
                SwapOutResult::Busy
            });
        }

        // Take the software dirty and referenced bits from the frame state
        let (software_dirty, software_referenced) = match &inner.state {
            SwappableFrameState::In { dirty, referenced, .. } => (*dirty, *referenced),
            SwappableFrameState::Out => return Err(SwapError::InvalidBacking),
            SwappableFrameState::Invalidating { .. } | SwappableFrameState::Invalid => {
                return Ok(SwapOutResult::Released);
            }
        };

        let (context, pte_access_dirty) = if inner.mapping_refs == 0 {
            (None, AccessDirty::default())
        } else {
            let inner = &*inner;
            let SwappableFrameState::In { frame, .. } = &inner.state else {
                unreachable!();
            };
            let (context, access_dirty) = inner.backend.inspect_access_dirty(frame);
            (Some(context), access_dirty)
        };

        let dirty = software_dirty | pte_access_dirty.dirty;
        if software_referenced || pte_access_dirty.accessed {
            let SwappableFrameState::In {
                dirty: state_dirty,
                referenced,
                ..
            } = &mut inner.state
            else {
                unreachable!();
            };
            *state_dirty = dirty;
            *referenced = false;
            return Ok(SwapOutResult::Referenced);
        }

        let final_pte_access_dirty = if let Some(context) = &context {
            let inner = &*inner;
            let SwappableFrameState::In { frame, .. } = &inner.state else {
                unreachable!();
            };
            inner.backend.unmap_no_flush(context, frame)
        } else {
            None
        };

        if final_pte_access_dirty.is_some() {
            arch::flush_tlb_all();
        }
        let dirty = dirty | final_pte_access_dirty.unwrap_or_default().dirty;

        let write_result = {
            let SwappableFrameInner {
                state,
                backend,
                mapping_refs,
                ..
            } = &mut *inner;
            let SwappableFrameState::In { frame, .. } = state else {
                unreachable!();
            };
            backend.write_out(frame, dirty, *mapping_refs)
        };

        if write_result.is_err() {
            let SwappableFrameState::In {
                dirty: state_dirty,
                referenced,
                ..
            } = &mut inner.state
            else {
                unreachable!();
            };
            *state_dirty = true;
            *referenced = false;
            return Ok(SwapOutResult::Failed);
        }

        let released = inner.mapping_refs == 0;
        let remove_backend = released.then(|| inner.backend.clone());
        inner.state = SwappableFrameState::Out;
        drop(inner);
        if let Some(backend) = remove_backend {
            backend.remove_unmapped_page(&self);
        }
        Ok(if released {
            SwapOutResult::Released
        } else {
            SwapOutResult::Evicted
        })
    }

    fn release_mapping_ref_locked(self: &Arc<Self>, inner: &mut SwappableFrameInner<B>) -> usize {
        debug_assert!(
            inner.mapping_refs > 0,
            "swappable frame mapping reference count underflow"
        );
        inner.mapping_refs -= 1;
        if inner.mapping_refs == 0 {
            match &inner.state {
                SwappableFrameState::Out if inner.backend.retain_unmapped_pages() => {}
                SwappableFrameState::In { .. } if inner.backend.retain_unmapped_pages() => {}
                _ if inner.backend.is_swappable() => {
                    let page: Arc<dyn SwappableFrameOps> = self.clone();
                    super::swapper::unlink(&page);
                }
                _ => {}
            }
        }
        if inner.mapping_refs == 0
            && matches!(inner.state, SwappableFrameState::In { .. })
            && inner.backend.retain_unmapped_pages()
            && inner.backend.is_swappable()
        {
            self.relink_locked(inner);
        }
        inner.mapping_refs
    }

    fn reclaim_class_locked(inner: &SwappableFrameInner<B>) -> ReclaimClass {
        if inner.mapping_refs != 0 {
            return ReclaimClass::Mapped;
        }
        match &inner.state {
            SwappableFrameState::In { dirty: true, .. } => ReclaimClass::DirtyUnmapped,
            SwappableFrameState::In { dirty: false, .. }
            | SwappableFrameState::Out
            | SwappableFrameState::Invalidating { .. }
            | SwappableFrameState::Invalid => ReclaimClass::CleanUnmapped,
        }
    }

    fn relink_locked(self: &Arc<Self>, inner: &SwappableFrameInner<B>) {
        let page: Arc<dyn SwappableFrameOps> = self.clone();
        super::swapper::relink(page, Self::reclaim_class_locked(inner));
    }
}

impl<B: SwappableBackendOps> SwappableFrameOps for SwappableFrame<B> {
    fn lru_node(&self) -> &IntrusiveLruNode<dyn SwappableFrameOps> {
        SwappableFrame::lru_node(self)
    }

    fn reclaim_class(&self) -> ReclaimClass {
        Self::reclaim_class_locked(&self.inner.lock())
    }

    fn try_swap_out(self: Arc<Self>) -> Result<SwapOutResult, SwapError> {
        let is_swappable = self.inner.lock().backend.is_swappable();
        if !is_swappable {
            return Ok(SwapOutResult::Released);
        }
        SwappableFrame::try_swap_out(self)
    }
}

impl<B: SwappableBackendOps> Drop for SwappableFrame<B> {
    fn drop(&mut self) {
        let mut inner = self.inner.lock();
        let SwappableFrameInner { state, backend, .. } = &mut *inner;
        match state {
            SwappableFrameState::In { .. } | SwappableFrameState::Out | SwappableFrameState::Invalid => {
                backend.release();
            }
            SwappableFrameState::Invalidating { .. } => {
                unreachable!("a swappable frame cannot be dropped while it is pinned")
            }
        }
    }
}

pub struct SwappableFrameGuard<'a, B: SwappableBackendOps> {
    owner: &'a Arc<SwappableFrame<B>>,
    inner: BucketSleepLockGuard<'a, SwappableFrameInner<B>>,
}

impl<B: SwappableBackendOps> SwappableFrameGuard<'_, B> {
    pub fn frame(&self) -> &PhysPageFrame {
        let inner = &self.inner;
        let SwappableFrameState::In { frame, .. } = &inner.state else {
            unreachable!();
        };
        frame
    }

    pub fn mark_dirty(&mut self) {
        let inner = &mut self.inner;
        let is_swappable = inner.backend.is_swappable();
        let SwappableFrameState::In { dirty, .. } = &mut inner.state else {
            unreachable!();
        };
        *dirty = true;
        if is_swappable {
            self.owner.relink_locked(inner);
        }
    }

    /// Takes ownership of the software dirty state collected before writeback.
    pub fn take_dirty(&mut self) -> bool {
        let inner = &mut self.inner;
        let is_swappable = inner.backend.is_swappable();
        let SwappableFrameState::In { dirty, .. } = &mut inner.state else {
            unreachable!();
        };
        let dirty = core::mem::replace(dirty, false);
        if is_swappable {
            self.owner.relink_locked(inner);
        }
        dirty
    }

    pub fn release_mapping_ref(&mut self) -> usize {
        let inner = &mut self.inner;
        self.owner.release_mapping_ref_locked(inner)
    }
}

impl<B: SwappableBackendOps> ResidentPageGuard for SwappableFrameGuard<'_, B> {
    fn frame(&self) -> &PhysPageFrame {
        SwappableFrameGuard::frame(self)
    }
}

impl<B: SwappableBackendOps> Drop for SwappableFrameGuard<'_, B> {
    fn drop(&mut self) {
        let inner = &mut self.inner;
        let is_swappable = inner.backend.is_swappable();
        let SwappableFrameState::In { referenced, .. } = &mut inner.state else {
            unreachable!();
        };
        *referenced = true;
        if is_swappable {
            self.owner.relink_locked(inner);
        }
    }
}

pub struct SwappableFramePin<B: SwappableBackendOps> {
    owner: Arc<SwappableFrame<B>>,
    kpage: usize,
}

impl<B: SwappableBackendOps> SwappableFramePin<B> {
    pub fn kpage(&self) -> usize {
        self.kpage
    }
}

impl<B: SwappableBackendOps> Drop for SwappableFramePin<B> {
    fn drop(&mut self) {
        let mut inner = self.owner.inner.lock();
        assert!(inner.pins > 0, "swappable frame pin count underflow");
        inner.pins -= 1;
        let last_pin = inner.pins == 0;
        let is_swappable = inner.backend.is_swappable();
        match &mut inner.state {
            SwappableFrameState::In { referenced, .. } => {
                *referenced = true;
                if is_swappable {
                    self.owner.relink_locked(&inner);
                }
            }
            SwappableFrameState::Invalidating { .. } if last_pin => {
                inner.state = SwappableFrameState::Invalid;
            }
            SwappableFrameState::Invalidating { .. } => {}
            SwappableFrameState::Out | SwappableFrameState::Invalid => {
                unreachable!("a pinned swappable frame must retain its physical frame")
            }
        }
    }
}
