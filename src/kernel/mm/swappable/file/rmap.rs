use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::num::NonZeroU64;

use crate::arch;
use crate::kernel::mm::AddrSpace;
use crate::klib::SpinLock;

use super::super::swappable::{TlbInvalidationLock, TlbInvalidationToken};

/* The way that a file mapping page to find the corresponding PTE
`state.backend.rmap` -> list of `FileMapEntry`
                     -> if `page.page_index` in `entry.file_page_base`..`entry.file_page_base` + `entry.page_count`
                     -> list of `AddrSpace` and `ubase` for the page
                     -> `AddrSpace` -> `ubase` -> PTE
 */

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MappingId(NonZeroU64);

#[derive(Clone)]
pub struct FileMapEntry {
    id: MappingId,
    addrspace: Weak<AddrSpace>,
    ubase: usize,
    file_page_base: usize,
    page_count: usize,
}

impl FileMapEntry {
    pub(super) fn addrspace(&self) -> &Weak<AddrSpace> {
        &self.addrspace
    }

    pub(super) fn page_uaddr(&self, file_page_index: usize) -> Option<usize> {
        let offset = file_page_index.checked_sub(self.file_page_base)?;
        if offset >= self.page_count {
            return None;
        }

        self.ubase.checked_add(offset.checked_mul(arch::PGSIZE)?)
    }
}

pub(super) struct FileRmap {
    inner: SpinLock<FileRmapInner>,
    tlb_invalidation_lock: TlbInvalidationLock,
}

struct FileRmapInner {
    next_id: NonZeroU64,
    entries: Arc<[FileMapEntry]>,
}

impl FileRmapInner {
    fn allocate_id(&mut self) -> MappingId {
        let id = MappingId(self.next_id);
        self.next_id = NonZeroU64::new(self.next_id.get().checked_add(1).expect("FileRmap MappingId overflow"))
            .expect("FileRmap MappingId must be non-zero");
        id
    }
}

impl FileRmap {
    pub(super) fn new() -> Self {
        Self {
            inner: SpinLock::new(
                FileRmapInner {
                    next_id: NonZeroU64::new(1).expect("initial MappingId must be non-zero"),
                    entries: Arc::from(Vec::<FileMapEntry>::new().into_boxed_slice()),
                },
                "FileRmap::inner",
            ),
            tlb_invalidation_lock: TlbInvalidationLock::new("FileRmap::tlb_invalidation_lock"),
        }
    }

    fn insert(&self, addrspace: Weak<AddrSpace>, ubase: usize, file_page_base: usize, page_count: usize) -> MappingId {
        Self::check_range(ubase, file_page_base, page_count);

        let mut inner = self.inner.lock();
        let id = inner.allocate_id();
        let mut entries = inner.entries.as_ref().to_vec();
        entries.push(FileMapEntry {
            id,
            addrspace,
            ubase,
            file_page_base,
            page_count,
        });
        inner.entries = Arc::from(entries.into_boxed_slice());
        id
    }

    fn update(&self, id: MappingId, ubase: usize, file_page_base: usize, page_count: usize) {
        Self::check_range(ubase, file_page_base, page_count);

        let mut inner = self.inner.lock();
        let mut entries = inner.entries.as_ref().to_vec();
        let entry = entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .expect("FileRmap entry is not registered");
        entry.ubase = ubase;
        entry.file_page_base = file_page_base;
        entry.page_count = page_count;
        inner.entries = Arc::from(entries.into_boxed_slice());
    }

    fn remove(&self, id: MappingId) {
        let mut inner = self.inner.lock();
        let mut entries = inner.entries.as_ref().to_vec();
        let index = entries
            .iter()
            .position(|entry| entry.id == id)
            .expect("FileRmap entry is not registered");
        entries.swap_remove(index);
        inner.entries = Arc::from(entries.into_boxed_slice());
    }

    fn split(
        &self,
        id: MappingId,
        left_ubase: usize,
        left_file_page_base: usize,
        left_page_count: usize,
        right_ubase: usize,
        right_file_page_base: usize,
        right_page_count: usize,
    ) -> MappingId {
        Self::check_range(left_ubase, left_file_page_base, left_page_count);
        Self::check_range(right_ubase, right_file_page_base, right_page_count);

        let mut inner = self.inner.lock();
        let right_id = inner.allocate_id();
        let mut entries = inner.entries.as_ref().to_vec();
        let left = entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .expect("FileRmap entry is not registered");

        left.ubase = left_ubase;
        left.file_page_base = left_file_page_base;
        left.page_count = left_page_count;

        let addrspace = left.addrspace.clone();
        entries.push(FileMapEntry {
            id: right_id,
            addrspace,
            ubase: right_ubase,
            file_page_base: right_file_page_base,
            page_count: right_page_count,
        });
        inner.entries = Arc::from(entries.into_boxed_slice());
        right_id
    }

    pub(super) fn snapshot(&self) -> Arc<[FileMapEntry]> {
        self.inner.lock().entries.clone()
    }

    fn check_range(ubase: usize, file_page_base: usize, page_count: usize) {
        file_page_base
            .checked_add(page_count)
            .expect("FileRmap page range overflow");
        let byte_count = page_count
            .checked_mul(arch::PGSIZE)
            .expect("FileRmap byte range overflow");
        ubase.checked_add(byte_count).expect("FileRmap address range overflow");
    }
}

pub struct FileMapRegistration {
    rmap: Arc<FileRmap>,
    id: Option<MappingId>,
}

impl FileMapRegistration {
    pub(super) fn new(rmap: Arc<FileRmap>) -> Self {
        Self { rmap, id: None }
    }

    pub fn is_bound(&self) -> bool {
        self.id.is_some()
    }

    pub fn with_tlb_invalidation_batch<R>(&self, f: impl FnOnce(&TlbInvalidationToken) -> R) -> R {
        self.rmap.tlb_invalidation_lock.with_batch(f)
    }

    pub fn bind(&mut self, addrspace: &Arc<AddrSpace>, ubase: usize, file_page_base: usize, page_count: usize) {
        assert!(self.id.is_none(), "file map range is already bound");
        self.id = Some(
            self.rmap
                .insert(Arc::downgrade(addrspace), ubase, file_page_base, page_count),
        );
    }

    pub fn update(&self, ubase: usize, file_page_base: usize, page_count: usize) {
        let id = self.id.expect("file map range is not bound");
        self.rmap.update(id, ubase, file_page_base, page_count);
    }

    pub fn fork(&self, addrspace: &Arc<AddrSpace>, ubase: usize, file_page_base: usize, page_count: usize) -> Self {
        let mut registration = Self::new(self.rmap.clone());
        registration.bind(addrspace, ubase, file_page_base, page_count);
        registration
    }

    pub fn split(
        &mut self,
        left_ubase: usize,
        left_file_page_base: usize,
        left_page_count: usize,
        right_ubase: usize,
        right_file_page_base: usize,
        right_page_count: usize,
    ) -> Self {
        let id = self.id.expect("file map range is not bound");
        let right_id = self.rmap.split(
            id,
            left_ubase,
            left_file_page_base,
            left_page_count,
            right_ubase,
            right_file_page_base,
            right_page_count,
        );
        Self {
            rmap: self.rmap.clone(),
            id: Some(right_id),
        }
    }

    pub fn unregister(&mut self) {
        if let Some(id) = self.id.take() {
            self.rmap.remove(id);
        }
    }
}

impl Drop for FileMapRegistration {
    fn drop(&mut self) {
        self.unregister();
    }
}
