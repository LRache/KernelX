use alloc::string::String;

pub enum DeviceType {
    Block,
    Char,
    Network,
    Other,
}

pub struct Device {
    pub addr: usize,
    pub typename: String,
    pub compatible: String,
}

impl Device {    
    fn addr(&self) -> usize {
        self.addr
    }
    
    fn typename(&self) -> &str {
        &self.typename
    }

    fn compatible(&self) -> &str {
        &self.compatible
    }
}


