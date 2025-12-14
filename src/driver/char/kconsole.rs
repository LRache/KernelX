// use alloc::sync::Arc;

// use crate::driver::{CharDriverOps, chosen::kconsole::KConsole};

// pub struct CharDriverConsole {
//     driver: Arc<dyn CharDriverOps>
// }

// impl KConsole for CharDriverConsole {
//     fn kputs(&self, s: &str) {
//         for byte in s.bytes() {
//             self.driver.putchar(byte);
//         }
//     }
// }
