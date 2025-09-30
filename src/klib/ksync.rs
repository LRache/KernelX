// pub trait SyncNoIRQHelper {
//     fn enable_irq();
//     fn disable_irq();
// }

// pub struct MutexNoIRQ<T> {
//     inner: T,
//     lock: arch::
// }

// impl<T> MutexNoIRQ<T> {
//     pub const fn new(data: T) -> Self {
//         Self {
//             inner: spin::Mutex::new(data),
//         }
//     }

//     pub fn lock(&self) -> spin::MutexGuard<'_, T> {
//         Helper::disable_irq();
//         let guard = self.inner.lock();
//         guard
//     }
// }
