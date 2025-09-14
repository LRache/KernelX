mod virtio;
mod manager;

pub mod block;

pub use manager::DEVICE_MANAGER as MANAGER;

pub fn init() {
    manager::init();
}
