use alloc::sync::Arc;
use bitvec::vec::BitVec;

use crate::arch;
use crate::driver::BlockDriverOps;
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::mm::swappable::swapper::{counter_swap_in, counter_swap_out};
use crate::klib::{InitedCell, SpinLock};

use super::super::SwapError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapSlot(usize);

pub struct SwapSpace {
    slots: SpinLock<BitVec>,
    driver: Arc<dyn BlockDriverOps>,
    blocks_per_page: usize,
}

impl SwapSpace {
    pub fn new(driver: Arc<dyn BlockDriverOps>) -> Result<Self, SwapError> {
        let block_size = usize::try_from(driver.get_block_size()).map_err(|_| SwapError::InvalidBacking)?;
        if block_size == 0 || arch::PGSIZE % block_size != 0 || driver.is_readonly() {
            return Err(SwapError::InvalidBacking);
        }

        let block_count = usize::try_from(driver.get_block_count()).map_err(|_| SwapError::InvalidBacking)?;
        let device_size = block_count.checked_mul(block_size).ok_or(SwapError::InvalidBacking)?;
        let slot_count = device_size / arch::PGSIZE;
        if slot_count == 0 {
            return Err(SwapError::InvalidBacking);
        }

        let blocks_per_page = arch::PGSIZE / block_size;
        crate::kinfo!(
            "Anonymous swapper: {} pages ({} KB) available.",
            slot_count,
            slot_count * arch::PGSIZE / 1024
        );

        Ok(Self {
            slots: SpinLock::new(BitVec::repeat(false, slot_count), "SwapSpace::slots"),
            driver,
            blocks_per_page,
        })
    }

    pub fn alloc_slot(&self) -> Result<SwapSlot, SwapError> {
        let mut slots = self.slots.lock();
        let slot = slots.first_zero().ok_or(SwapError::NoSpace)?;
        slots.set(slot, true);
        Ok(SwapSlot(slot))
    }

    pub fn free_slot(&self, slot: SwapSlot) {
        let mut slots = self.slots.lock();
        assert!(slot.0 < slots.len(), "swap slot is out of bounds");
        assert!(slots[slot.0], "swap slot is already free");
        slots.set(slot.0, false);
    }

    pub fn read_page(&self, slot: SwapSlot, frame: &PhysPageFrame) -> Result<(), SwapError> {
        let start_block = self.start_block(slot)?;
        let buffer = frame.slice();
        if buffer.len() != arch::PGSIZE {
            return Err(SwapError::InvalidBacking);
        }

        let start = crate::kernel::event::timer::now();
        self.driver
            .read_blocks(start_block, buffer)
            .map_err(|_| SwapError::Io)?;
        counter_swap_in(crate::kernel::event::timer::now() - start);
        Ok(())
    }

    pub fn write_page(&self, slot: SwapSlot, frame: &PhysPageFrame) -> Result<(), SwapError> {
        let start_block = self.start_block(slot)?;
        let buffer = frame.slice();
        if buffer.len() != arch::PGSIZE {
            return Err(SwapError::InvalidBacking);
        }

        let start = crate::kernel::event::timer::now();
        self.driver
            .write_blocks(start_block, buffer)
            .map_err(|_| SwapError::Io)?;
        counter_swap_out(crate::kernel::event::timer::now() - start);
        Ok(())
    }

    fn start_block(&self, slot: SwapSlot) -> Result<usize, SwapError> {
        {
            let slots = self.slots.lock();
            if slot.0 >= slots.len() || !slots[slot.0] {
                return Err(SwapError::InvalidBacking);
            }
        }

        let start_block = slot
            .0
            .checked_mul(self.blocks_per_page)
            .ok_or(SwapError::InvalidBacking)?;
        let end_block = start_block
            .checked_add(self.blocks_per_page)
            .ok_or(SwapError::InvalidBacking)?;
        let block_count = usize::try_from(self.driver.get_block_count()).map_err(|_| SwapError::InvalidBacking)?;
        if end_block > block_count {
            return Err(SwapError::InvalidBacking);
        }

        Ok(start_block)
    }
}

static SWAP_SPACE: InitedCell<Option<SwapSpace>> = InitedCell::uninit();

pub fn init_swap_space(driver: Option<Arc<dyn BlockDriverOps>>) {
    let swap_space = driver
        .map(|driver| match SwapSpace::new(driver) {
            Ok(swap_space) => Some(swap_space),
            Err(_) => {
                crate::kwarn!("Anonymous swap space initialization failed.");
                None
            }
        })
        .flatten();
    if swap_space.is_none() {
        crate::kinfo!("Anonymous swap disabled.");
    }
    SWAP_SPACE.init(swap_space);
}

pub fn swap_space() -> Option<&'static SwapSpace> {
    SWAP_SPACE.try_get().and_then(Option::as_ref)
}
