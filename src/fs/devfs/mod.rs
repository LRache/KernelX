mod superblock;
mod null;
mod zero;
mod devnode;

use null::NullInode;
use zero::ZeroInode;

pub use superblock::FileSystem;
pub use superblock::{init, add_device};
