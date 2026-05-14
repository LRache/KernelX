use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy)]
    pub struct CapabilitySet: u32 {
        const CHOWN = 1 << 0;
        const KILL = 1 << 5;
        const SETPCAP = 1 << 8;
        const NET_RAW = 1 << 13;
        const SYS_TIME = 1 << 25;
    }
}

impl CapabilitySet {
    pub fn from_cap_number(capability: usize) -> Option<Self> {
        if capability < u32::BITS as usize {
            Some(Self::from_bits_retain(1u32 << capability))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy)]
pub struct ProcessCapabilities {
    pub effective: CapabilitySet,
    pub permitted: CapabilitySet,
    pub inheritable: CapabilitySet,
    pub bounding: CapabilitySet,
}

impl ProcessCapabilities {
    pub fn init() -> Self {
        let initial = CapabilitySet::from_bits_retain(u32::MAX);
        Self {
            effective: initial,
            permitted: initial,
            inheritable: CapabilitySet::empty(),
            bounding: initial,
        }
    }
}
