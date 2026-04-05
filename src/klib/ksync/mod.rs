cfg_if::cfg_if! {
    if #[cfg(feature = "nolock")] {
        mod nolock;
        pub use nolock::NoLockMutex as SpinLock;
        pub use nolock::NoLockMutex as SleepLock;
    } else {
        mod locker;
        mod mutex;
        mod spinlock;
        mod sleeplock;

        #[cfg(feature = "deadlock-detect")]
        mod lockdep;
        #[cfg(feature = "deadlock-detect")]
        pub use lockdep::LockState;

        pub use spinlock::SpinLock;
        pub use sleeplock::SleepLock;
    }
}

mod tasklocal;

pub use tasklocal::TaskLocal;
