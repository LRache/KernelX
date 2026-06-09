cfg_if::cfg_if! {
    if #[cfg(feature = "nolock")] {
        mod nolock;
        pub use nolock::NoLockMutex as SpinLock;
        pub use nolock::NoLockRWLock as RWLock;
        pub use nolock::NoLockMutex as SleepLock;
    } else {
        mod locker;
        mod mutex;
        mod rwlock;
        mod spinlock;
        mod sleeplock;

        #[cfg(feature = "lockdep")]
        mod lockdep;
        #[cfg(feature = "lockdep")]
        pub use lockdep::LockState;

        pub use rwlock::RWLock;
        pub use spinlock::SpinLock;
        pub use sleeplock::SleepLock;
    }
}

mod tasklocal;

pub use tasklocal::TaskLocal;
