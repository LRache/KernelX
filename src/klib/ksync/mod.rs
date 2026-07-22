mod bucket_sleeplock;
mod locker;
mod mutex;
#[cfg(feature = "nolock")]
mod nolock;
#[cfg(not(feature = "nolock"))]
mod rwlock;
mod sleeplock;
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
pub use sleeplock::{SleepLock, SleepLockGuard};
#[cfg(not(feature = "nolock"))]
pub use spinlock::SpinLock;
pub use tasklocal::TaskLocal;
