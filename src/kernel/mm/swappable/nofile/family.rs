use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::num::NonZeroU64;

use crate::arch;
use crate::kernel::mm::AddrSpace;
use crate::klib::SpinLock;

use super::super::swappable::{TlbInvalidationLock, TlbInvalidationToken};

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappingId(NonZeroU64);

#[derive(Clone)]
pub struct AnonMapFamilyMember {
    id: MappingId,
    addrspace: Weak<AddrSpace>,
    ubase: usize,

    /// The index of the first page in the mapping range.
    /// If an area is splited from parent, the child's `page_base` may not be zero.
    page_base: usize,

    /// The number of pages in the mapping range.
    /// The mapping range is [page_base, page_base + page_count).
    page_count: usize,
}

impl AnonMapFamilyMember {
    pub fn addrspace(&self) -> &Weak<AddrSpace> {
        &self.addrspace
    }

    pub fn page_uaddr(&self, page_index: usize) -> Option<usize> {
        let offset = page_index.checked_sub(self.page_base)?;
        if offset >= self.page_count {
            return None;
        }

        self.ubase.checked_add(offset.checked_mul(arch::PGSIZE)?)
    }
}

/// `AnonMapFamily` records a family of anonymously mapped `MapArea`s.
/// A child `MapArea` forked or split from its parent shares the same `AnonMapFamily`,
/// and each `MapArea` has a unique `MappingId` in the family.
/// The anonymous page frame uses the `AnonMapFamily` to find all the `MapArea`s that map it.
pub struct AnonMapFamily {
    inner: SpinLock<AnonMapFamilyInner>,
    tlb_invalidation_lock: TlbInvalidationLock,
}

struct AnonMapFamilyInner {
    next_id: NonZeroU64,
    members: Arc<[AnonMapFamilyMember]>,
}

impl AnonMapFamilyInner {
    fn allocate_id(&mut self) -> MappingId {
        let id = MappingId(self.next_id);
        self.next_id = NonZeroU64::new(
            self.next_id
                .get()
                .checked_add(1)
                .expect("AnonMapFamily MappingId overflow"),
        )
        .expect("AnonMapFamily MappingId must be non-zero");
        id
    }
}

impl AnonMapFamily {
    pub fn new() -> Self {
        Self {
            inner: SpinLock::new(
                AnonMapFamilyInner {
                    next_id: NonZeroU64::new(1).expect("initial MappingId must be non-zero"),
                    members: Arc::from(Vec::<AnonMapFamilyMember>::new().into_boxed_slice()),
                },
                "AnonMapFamily::inner",
            ),
            tlb_invalidation_lock: TlbInvalidationLock::new("AnonMapFamily::tlb_invalidation_lock"),
        }
    }

    pub fn insert(&self, addrspace: Weak<AddrSpace>, ubase: usize, page_base: usize, page_count: usize) -> MappingId {
        Self::check_range(ubase, page_base, page_count);

        let mut inner = self.inner.lock();
        let id = inner.allocate_id();

        let mut members = inner.members.as_ref().to_vec();
        members.push(AnonMapFamilyMember {
            id,
            addrspace,
            ubase,
            page_base,
            page_count,
        });
        inner.members = Arc::from(members.into_boxed_slice());
        id
    }

    pub fn update(&self, id: MappingId, ubase: usize, page_base: usize, page_count: usize) {
        Self::check_range(ubase, page_base, page_count);

        let mut inner = self.inner.lock();
        let mut members = inner.members.as_ref().to_vec();
        let member = members
            .iter_mut()
            .find(|member| member.id == id)
            .expect("AnonMapFamily member is not registered");
        member.ubase = ubase;
        member.page_base = page_base;
        member.page_count = page_count;
        inner.members = Arc::from(members.into_boxed_slice());
    }

    pub fn remove(&self, id: MappingId) {
        let mut inner = self.inner.lock();
        let mut members = inner.members.as_ref().to_vec();
        let index = members
            .iter()
            .position(|member| member.id == id)
            .expect("AnonMapFamily member is not registered");
        members.swap_remove(index);
        inner.members = Arc::from(members.into_boxed_slice());
    }

    fn split(
        &self,
        id: MappingId,
        left_ubase: usize,
        left_page_base: usize,
        left_page_count: usize,
        right_ubase: usize,
        right_page_base: usize,
        right_page_count: usize,
    ) -> MappingId {
        Self::check_range(left_ubase, left_page_base, left_page_count);
        Self::check_range(right_ubase, right_page_base, right_page_count);

        let mut inner = self.inner.lock();
        let right_id = inner.allocate_id();

        let mut members = inner.members.as_ref().to_vec();
        let left = members
            .iter_mut()
            .find(|member| member.id == id)
            .expect("AnonMapFamily member is not registered");

        left.ubase = left_ubase;
        left.page_base = left_page_base;
        left.page_count = left_page_count;

        let addrspace = left.addrspace.clone();
        members.push(AnonMapFamilyMember {
            id: right_id,
            addrspace,
            ubase: right_ubase,
            page_base: right_page_base,
            page_count: right_page_count,
        });
        inner.members = Arc::from(members.into_boxed_slice());

        right_id
    }

    pub fn snapshot(&self) -> Arc<[AnonMapFamilyMember]> {
        self.inner.lock().members.clone()
    }

    fn check_range(ubase: usize, page_base: usize, page_count: usize) {
        page_base
            .checked_add(page_count)
            .expect("AnonMapFamily page range overflow");
        let byte_count = page_count
            .checked_mul(arch::PGSIZE)
            .expect("AnonMapFamily byte range overflow");
        ubase
            .checked_add(byte_count)
            .expect("AnonMapFamily address range overflow");
    }
}

pub struct AnonMapFamilyRegistration {
    mapping: Arc<AnonMapFamily>,

    /// The unique identifier of the mapping in the `AnonMapFamily`.
    id: Option<MappingId>,
}

impl AnonMapFamilyRegistration {
    pub fn new_root() -> Self {
        Self::new(Arc::new(AnonMapFamily::new()))
    }

    fn new(mapping: Arc<AnonMapFamily>) -> Self {
        Self { mapping, id: None }
    }

    pub(super) fn mapping(&self) -> Arc<AnonMapFamily> {
        self.mapping.clone()
    }

    pub fn is_bound(&self) -> bool {
        self.id.is_some()
    }

    pub fn with_tlb_invalidation_batch<R>(&self, f: impl FnOnce(&TlbInvalidationToken) -> R) -> R {
        self.mapping.tlb_invalidation_lock.with_batch(f)
    }

    pub fn unbound_sibling(&self) -> Self {
        Self::new(self.mapping.clone())
    }

    pub fn fork(&self, addrspace: &Arc<AddrSpace>, ubase: usize, page_base: usize, page_count: usize) -> Self {
        let mut range = Self::new(self.mapping.clone());
        range.bind(addrspace, ubase, page_base, page_count);
        range
    }

    pub fn bind(&mut self, addrspace: &Arc<AddrSpace>, ubase: usize, page_base: usize, page_count: usize) {
        assert!(self.id.is_none(), "anonymous map range is already bound");
        self.id = Some(
            self.mapping
                .insert(Arc::downgrade(addrspace), ubase, page_base, page_count),
        );
    }

    pub fn update(&self, ubase: usize, page_base: usize, page_count: usize) {
        let id = self.id.expect("anonymous map range is not bound");
        self.mapping.update(id, ubase, page_base, page_count);
    }

    pub fn split(
        &mut self,
        left_ubase: usize,
        left_page_base: usize,
        left_page_count: usize,
        right_ubase: usize,
        right_page_base: usize,
        right_page_count: usize,
    ) -> Self {
        let id = self.id.expect("anonymous map range is not bound");
        let right_id = self.mapping.split(
            id,
            left_ubase,
            left_page_base,
            left_page_count,
            right_ubase,
            right_page_base,
            right_page_count,
        );
        Self {
            mapping: self.mapping.clone(),
            id: Some(right_id),
        }
    }

    pub fn unregister(&mut self) {
        if let Some(id) = self.id.take() {
            self.mapping.remove(id);
        }
    }
}

impl Drop for AnonMapFamilyRegistration {
    fn drop(&mut self) {
        self.unregister();
    }
}
