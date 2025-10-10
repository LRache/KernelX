use crate::kernel::api;
use crate::kernel::ipc::SignalAction;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Sigaction {
    pub sa_handler:   usize,
    pub sa_sigaction: usize,
    pub sa_mask: api::sigset_t,
    pub sa_flags: i32,
    pub sa_restorer: usize,
}

impl Sigaction {
    pub fn empty() -> Self {
        Sigaction {
            sa_handler: 0,
            sa_sigaction: 0,
            sa_mask: 0,
            sa_flags: 0,
            sa_restorer: 0,
        }
    }
}

impl Into<SignalAction> for Sigaction {
    fn into(self) -> SignalAction {
        SignalAction {
            handler: self.sa_handler,
            restorer: self.sa_restorer,
            mask: self.sa_mask,
        }
    }
}

impl From<SignalAction> for Sigaction {
    fn from(item: SignalAction) -> Self {
        Sigaction {
            sa_handler: item.handler,
            sa_sigaction: item.handler,
            sa_mask: item.mask,
            sa_flags: 0,
            sa_restorer: item.restorer,
        }
    }
}
