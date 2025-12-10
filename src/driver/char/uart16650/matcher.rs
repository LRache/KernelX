use alloc::sync::Arc;

use crate::driver::matcher::DriverMatcher;
use crate::driver::{Device, DriverOps};


pub struct Matcher;

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        None
    }
}
