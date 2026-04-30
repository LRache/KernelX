use alloc::sync::Arc;

use crate::klib::RWLock;

pub const UTS_NAME_MAX: usize = 64;

const DEFAULT_HOSTNAME: &[u8] = b"kernelx";
const DEFAULT_DOMAINNAME: &[u8] = b"none";

#[derive(Clone, Copy)]
struct UtsName {
    bytes: [u8; UTS_NAME_MAX],
    len: usize,
}

impl UtsName {
    fn new(bytes: &[u8]) -> Self {
        let mut name = Self {
            bytes: [0; UTS_NAME_MAX],
            len: bytes.len(),
        };
        name.bytes[..bytes.len()].copy_from_slice(bytes);
        name
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[derive(Clone, Copy)]
struct UtsNamespaceInner {
    hostname: UtsName,
    domainname: UtsName,
}

#[derive(Clone)]
pub struct UtsNamespace {
    inner: Arc<RWLock<UtsNamespaceInner>>,
}

impl UtsNamespace {
    pub(super) fn new() -> Self {
        Self {
            inner: Arc::new(RWLock::new(
                UtsNamespaceInner {
                    hostname: UtsName::new(DEFAULT_HOSTNAME),
                    domainname: UtsName::new(DEFAULT_DOMAINNAME),
                },
                "UtsNamespace::inner",
            )),
        }
    }

    pub(super) fn fork(&self) -> Self {
        Self {
            inner: Arc::new(RWLock::new(*self.inner.read(), "UtsNamespace::inner")),
        }
    }

    pub fn write_hostname_to(&self, dst: &mut [u8]) -> usize {
        let inner = self.inner.read();
        dst[..inner.hostname.len].copy_from_slice(inner.hostname.as_bytes());
        inner.hostname.len
    }

    pub fn write_domainname_to(&self, dst: &mut [u8]) -> usize {
        let inner = self.inner.read();
        dst[..inner.domainname.len].copy_from_slice(inner.domainname.as_bytes());
        inner.domainname.len
    }

    pub fn set_hostname(&self, bytes: &[u8]) {
        self.inner.write().hostname = UtsName::new(bytes);
    }

    pub fn set_domainname(&self, bytes: &[u8]) {
        self.inner.write().domainname = UtsName::new(bytes);
    }
}
