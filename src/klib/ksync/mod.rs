mod bucket_sleeplock;
mod locker;
mod mutex;
#[cfg(feature = "nolock")]
mod nolock;
#[allow(dead_code)]
mod onstack_waiter;
#[cfg(not(feature = "nolock"))]
mod rwlock;
#[allow(dead_code)]
mod sleep_rwlock_onstack;
mod sleeplock;
#[allow(dead_code)]
mod sleeplock_onstack;
#[cfg(not(feature = "nolock"))]
mod spinlock;
mod tasklocal;

#[cfg(feature = "lockdep")]
mod lockdep;
pub use bucket_sleeplock::{BucketSleepLock, BucketSleepLockGuard};
#[cfg(feature = "lockdep")]
pub use lockdep::LockState;
#[cfg(feature = "nolock")]
pub use nolock::NoLockMutex as SpinLock;
#[cfg(feature = "nolock")]
pub use nolock::NoLockRWLock as RWLock;
#[cfg(not(feature = "nolock"))]
pub use rwlock::RWLock;
#[allow(unused_imports)]
pub use sleep_rwlock_onstack::{SleepRwLockOnStack, SleepRwLockOnStackReadGuard, SleepRwLockOnStackWriteGuard};
pub use sleeplock::{SleepLock, SleepLockGuard};
#[allow(unused_imports)]
pub use sleeplock_onstack::{SleepLockOnStack, SleepLockOnStackGuard};
#[cfg(not(feature = "nolock"))]
pub use spinlock::SpinLock;
pub use tasklocal::TaskLocal;
