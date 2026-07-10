#[cfg(not(feature = "fanotify"))]
mod disabled;
#[cfg(feature = "fanotify")]
mod file;
#[cfg(feature = "fanotify")]
mod mark;
#[cfg(feature = "fanotify")]
mod notify;
mod types;

#[cfg(not(feature = "fanotify"))]
pub use disabled::{
    notify_fanotify, notify_fanotify_dentry, wait_fanotify_open_exec_permission, wait_fanotify_permission,
};
#[cfg(feature = "fanotify")]
pub use file::FanotifyFile;
#[cfg(feature = "fanotify")]
pub use mark::Fanotify;
#[cfg(feature = "fanotify")]
pub use notify::{
    notify_fanotify, notify_fanotify_dentry, wait_fanotify_open_exec_permission, wait_fanotify_permission,
};
pub use types::FanotifyEventMask;
#[cfg(feature = "fanotify")]
pub use types::{FanotifyFdinfoKey, FanotifyMarkFlags};
