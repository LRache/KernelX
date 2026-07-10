use num_enum::TryFromPrimitive;

use crate::arch::VCpu;
use crate::print;

#[repr(usize)]
#[derive(Clone, Copy, Debug, TryFromPrimitive)]
enum SbiEid {
    LegacySetTimer = 0x0,
    LegacyConsolePutchar = 0x1,
    Time = 0x54494d45,
}

#[repr(usize)]
#[derive(Clone, Copy, Debug, TryFromPrimitive)]
enum SbiTimeFid {
    SetTimer = 0x0,
}

const SBI_SUCCESS: usize = 0;
const SBI_ERR_NOT_SUPPORTED: usize = -2isize as usize;

const REG_A0: usize = 10;
const REG_A1: usize = 11;
const REG_A6: usize = 16;
const REG_A7: usize = 17;
const INST_SIZE: usize = 4;

impl VCpu {
    pub fn handle_sbi_call(&self) -> bool {
        let mut state = self.state.lock();
        let gpr = state.gpr();
        let Ok(eid) = SbiEid::try_from(gpr[REG_A7]) else {
            return false;
        };
        let fid = gpr[REG_A6];
        let a0 = gpr[REG_A0];

        let (a0, a1) = match eid {
            SbiEid::LegacySetTimer => {
                state.set_vstimecmp(a0);
                (SBI_SUCCESS, 0)
            }
            SbiEid::LegacyConsolePutchar => {
                print!("{}", a0 as u8 as char);
                (SBI_SUCCESS, 0)
            }
            SbiEid::Time => match SbiTimeFid::try_from(fid) {
                Ok(SbiTimeFid::SetTimer) => {
                    state.set_vstimecmp(a0);
                    (SBI_SUCCESS, 0)
                }
                _ => (SBI_ERR_NOT_SUPPORTED, 0),
            },
        };

        state.gpr_mut()[REG_A0] = a0;
        state.gpr_mut()[REG_A1] = a1;
        state.pc += INST_SIZE;

        return true;
    }
}
