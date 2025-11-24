use crate::kernel::config;

#[derive(Clone)]
pub struct UserBrk {
    pub page_count: usize,
    pub ubrk: usize,
}

impl UserBrk {
    pub fn new() -> Self {
        UserBrk {
            page_count: 0,
            ubrk: config::USER_BRK_BASE,
        }
    }
}
