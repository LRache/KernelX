use crate::kernel::errno::Errno;
use crate::kernel::errno::SysResult;
use crate::kernel::ipc::SignalAction;
use crate::kernel::ipc::SignalActionFlags;
use crate::kernel::ipc::SignalSet;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Sigaction {
    pub sa_handler: usize,
    pub sa_flags: u32,
    pub sa_mask: SignalSet,
}

impl TryInto<SignalAction> for Sigaction {
    type Error = Errno;
    fn try_into(self) -> SysResult<SignalAction> {
        Ok(SignalAction {
            handler: self.sa_handler,
            mask: self.sa_mask,
            flags: SignalActionFlags::from_bits(self.sa_flags as u32).ok_or(Errno::EINVAL)?,
        })
    }
}

impl From<SignalAction> for Sigaction {
    fn from(item: SignalAction) -> Self {
        Sigaction {
            sa_handler: item.handler,
            sa_mask: item.mask,
            sa_flags: item.flags.bits(),
        }
    }
}
