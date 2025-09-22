mod inner;
mod device;
mod driver;

use inner::VirtIOBlockDriverInner;
pub use device::VirtIOBlockDevice;
pub use driver::VirtIOBlockDriver;
