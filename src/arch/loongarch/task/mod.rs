//! Per-task arch support: kernel-context switch + user-trap dispatcher.

pub mod switch;
pub mod traphandle;

// Re-export so `src/arch/loongarch/mod.rs`'s `pub use task::kernel_switch`
// keeps working after the module got reshaped from a single file into a
// directory.
pub use switch::kernel_switch;
