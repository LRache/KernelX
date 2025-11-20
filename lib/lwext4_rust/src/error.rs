use core::{
    error::Error,
    fmt::{Debug, Display},
};

use crate::ffi::EOK;

pub type Ext4Result<T = ()> = Result<T, Ext4Error>;

pub struct Ext4Error {
    pub code: i32,
    pub context: Option<&'static str>,
}
impl Ext4Error {
    pub fn new(code: i32, context: impl Into<Option<&'static str>>) -> Self {
        Ext4Error {
            code,
            context: context.into(),
        }
    }
}

impl From<i32> for Ext4Error {
    fn from(code: i32) -> Self {
        Ext4Error::new(code, None)
    }
}

impl Display for Ext4Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if let Some(context) = self.context {
            write!(f, "ext4 error {}: {context}", self.code)
        } else {
            write!(f, "ext4 error {}", self.code)
        }
    }
}

impl Debug for Ext4Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Display::fmt(self, f)
    }
}

impl Error for Ext4Error {}

pub(crate) trait Context<T> {
    fn context(self, context: &'static str) -> Result<T, Ext4Error>;
}
impl Context<()> for i32 {
    fn context(self, context: &'static str) -> Result<(), Ext4Error> {
        if self != EOK as _ {
            Err(Ext4Error::new(self, Some(context)))
        } else {
            Ok(())
        }
    }
}
impl<T> Context<T> for Ext4Result<T> {
    fn context(self, context: &'static str) -> Result<T, Ext4Error> {
        self.map_err(|e| Ext4Error::new(e.code, Some(context)))
    }
}
