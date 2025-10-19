use crate::kernel::ipc::SignalAction;
use crate::kernel::ipc::SignalActionFlags;
use crate::kernel::ipc::SignalSet;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Sigaction {
    pub sa_handler: usize,
    pub sa_flags: usize,
    // pub sa_restorer: usize,
    pub sa_mask: SignalSet,
}

impl Into<SignalAction> for Sigaction {
    fn into(self) -> SignalAction {
        SignalAction {
            handler: self.sa_handler,
            // restorer: self.sa_restorer,
            mask: self.sa_mask,
            flags: SignalActionFlags::from_bits_truncate(self.sa_flags as u32),
        }
    }
}

impl From<SignalAction> for Sigaction {
    fn from(item: SignalAction) -> Self {
        Sigaction {
            sa_handler: item.handler,
            // sa_sigaction: item.handler,
            sa_mask: item.mask,
            sa_flags: 0,
            // sa_restorer: item.restorer,
        }
    }
}
