use crate::driver::chosen::kpmu::KPMU;
use crate::driver::PMUDriverOps;

use super::sbi;

pub struct SBIPMUDriver;

impl PMUDriverOps for SBIPMUDriver {
    fn shutdown(&self) -> ! {
        sbi::shutdown();
    }
}

pub struct SBIKPMU;

impl KPMU for SBIKPMU {
    fn shutdown(&self) -> ! {
        sbi::shutdown();
    }
}
