use core::mem::size_of;
use core::slice;

#[repr(C)]
pub struct KModuleExport {
    name: *const u8,
    name_len: usize,
    addr: *const (),
}

// SAFETY: KModuleExport entries are immutable records emitted into the
// .kmodule.exports section; sharing their raw function/name pointers between
// cores does not mutate the pointed-to data.
unsafe impl Sync for KModuleExport {}

impl KModuleExport {
    pub const fn new(name: &'static str, addr: *const ()) -> Self {
        Self {
            name: name.as_ptr(),
            name_len: name.len(),
            addr,
        }
    }

    fn name_bytes(&self) -> Option<&'static [u8]> {
        if self.name.is_null() {
            return None;
        }
        // SAFETY: KModuleExport::new stores a pointer and length from a
        // &'static str, and linker-provided export records are expected to keep
        // that exact invariant.
        Some(unsafe { slice::from_raw_parts(self.name, self.name_len) })
    }

    fn addr(&self) -> usize {
        self.addr as usize
    }
}

// SAFETY: These symbols are provided by the kernel linker script around the
// .kmodule.exports section and are only used through addr_of! below.
unsafe extern "C" {
    static __kmodule_exports_start: KModuleExport;
    static __kmodule_exports_end: KModuleExport;
}

pub fn resolve(name: &str) -> Option<usize> {
    exports()
        .iter()
        .find(|export| export.name_bytes() == Some(name.as_bytes()))
        .map(KModuleExport::addr)
}

fn exports() -> &'static [KModuleExport] {
    let start = core::ptr::addr_of!(__kmodule_exports_start) as usize;
    let end = core::ptr::addr_of!(__kmodule_exports_end) as usize;
    if end < start || !(end - start).is_multiple_of(size_of::<KModuleExport>()) {
        return &[];
    }

    // SAFETY: The linker script places only KModuleExport records between the
    // start/end symbols, and the range size was checked above.
    unsafe {
        slice::from_raw_parts(
            start as *const KModuleExport,
            (end - start) / size_of::<KModuleExport>(),
        )
    }
}

#[macro_export]
macro_rules! kmodule_export {
    ($name:literal, $func:path) => {
        const _: () = {
            #[used]
            // SAFETY: The loader discovers exported kernel symbols by walking
            // this linker-collected section.
            #[unsafe(link_section = ".kmodule.exports")]
            static EXPORT: $crate::kmodule::exports::KModuleExport =
                $crate::kmodule::exports::KModuleExport::new($name, $func as *const ());
        };
    };
}
