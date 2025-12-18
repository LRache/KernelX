use crate::driver::chosen::kpmu::KPMU;

use super::sbi;

pub struct SBIKPMU;

impl KPMU for SBIKPMU {
    fn shutdown(&self) -> ! {
        sbi::shutdown();
    }
}
