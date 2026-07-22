cfg_if::cfg_if! {
    if #[cfg(feature = "swap-memory")] {
        mod frame;
        mod family;
        mod swapspace;

        pub use frame::{AnonymousSwappableFrame, AnonymousSwappableFramePin};
        pub use family::AnonMapFamilyRegistration;
        pub use swapspace::*;
    } else {
        mod noswap;

        pub use noswap::{AnonMapFamilyRegistration, AnonymousSwappableFrame, AnonymousSwappableFramePin};
    }
}
