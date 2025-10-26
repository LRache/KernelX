// use alloc::string::String;
// use alloc::boxed::Box;

// use super::{BlockDevice, BlockDriver};

// pub struct EMMCDevice;

// impl BlockDevice for EMMCDevice {
//     fn name(&self) -> String {
//         "emmc".into()
//     }

//     fn driver(&self) -> Box<dyn BlockDriver> {
//         Box::new(EMMCDriver::new())
//     }
// }

// #[derive(Clone)]
// pub struct EMMCDriver {
//     base: usize,
// }

// impl EMMCDriver {
//     pub fn new() -> Self {
//         Self { base: 0x3F300000 } // Raspberry Pi 3
//     }
// }

// impl BlockDriver for EMMCDriver {
//     fn clone(&self) -> Box<dyn BlockDriver> {
//         Box::new(self.clone())
//     }

//     fn read_block(&mut self, _block: usize, _buf: &mut [u8]) -> Result<(), ()> {
//         unimplemented!()
//     }

//     fn write_block(&mut self, _block: usize, _buf: &[u8]) -> Result<(), ()> {
//         unimplemented!()
//     }

//     fn flush(&mut self) -> Result<(), ()> {
//         Ok(())
//     }

//     fn get_block_size(&self) -> u32 {
//         512
//     }

//     fn get_block_count(&self) -> u64 {
//         0
//     }
// }
