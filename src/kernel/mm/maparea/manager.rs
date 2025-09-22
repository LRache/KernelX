use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::RwLock;

use crate::{ktrace, print};
use crate::kernel::config;
use crate::kernel::errno::Errno;
use crate::arch::{self, PageTable};
use crate::kernel::mm::maparea::anonymous::AnonymousArea;
use crate::kernel::mm::{MapPerm, MemAccessType};
use super::area::Area;
use super::userstack::{UserStack, Auxv};
use super::userbrk::UserBrk;

pub struct Manager {
    areas: BTreeMap<usize, Box<dyn Area>>,
    userstack_ubase: usize,
    userbrk: UserBrk,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            areas: BTreeMap::new(),
            userstack_ubase: 0,
            userbrk: UserBrk::new()
        }
    }

    pub fn fork(&mut self, self_pagetable: &RwLock<PageTable>, new_pagetable: &RwLock<PageTable>) -> Manager {
        let new_areas = self.areas.iter_mut().map(|(ubase, area)| {
            (*ubase, area.fork(self_pagetable, new_pagetable))
        }).collect();
        
        Self {
            areas: new_areas,
            userstack_ubase: self.userstack_ubase,
            userbrk: self.userbrk.clone()
        }
    }

    /// Find a suitable virtual address for mmap allocation
    /// 
    /// Searches for a contiguous region of virtual memory that can accommodate
    /// the requested number of pages, starting from USER_MAP_BASE.
    /// 
    /// # Arguments
    /// * `page_count` - Number of pages to allocate
    /// 
    /// # Returns
    /// * `Some(usize)` - Base address for the allocation if found
    /// * `None` - If no suitable address space is available
    pub fn find_mmap_ubase(&self, page_count: usize) -> Option<usize> {
        if page_count == 0 {
            return None;
        }

        let required_size = page_count * arch::PGSIZE;
        let mut candidate_addr = config::USER_MAP_BASE;

        // Ensure candidate address is page-aligned
        candidate_addr = (candidate_addr + arch::PGSIZE - 1) & !(arch::PGSIZE - 1);

        // Iterate through existing areas in ascending order to find a gap
        for (&area_base, area) in &self.areas {
            let area_end = area_base + area.size();

            // Check if candidate address is before this area and has enough space
            if candidate_addr + required_size <= area_base {
                // Found a suitable gap before this area
                ktrace!("Found mmap address {:#x} for {} pages (gap before area at {:#x})", 
                       candidate_addr, page_count, area_base);
                return Some(candidate_addr);
            }

            // If candidate overlaps with or is too close to this area, 
            // move to after this area
            if candidate_addr < area_end {
                candidate_addr = area_end;
                // Re-align to page boundary
                candidate_addr = (candidate_addr + arch::PGSIZE - 1) & !(arch::PGSIZE - 1);
            }
        }

        // Check if there's space after all existing areas
        // We need to ensure we don't exceed reasonable address space limits
        // For safety, let's limit to addresses below the user stack region
        let max_mmap_addr = if self.userstack_ubase > 0 {
            self.userstack_ubase
        } else {
            config::USER_STACK_TOP - config::USER_STACK_PAGE_COUNT_MAX * arch::PGSIZE
        };

        if candidate_addr + required_size <= max_mmap_addr {
            ktrace!("Found mmap address {:#x} for {} pages (after all areas)", 
                   candidate_addr, page_count);
            Some(candidate_addr)
        } else {
            ktrace!("No suitable mmap address found for {} pages (would exceed limit {:#x})", 
                   page_count, max_mmap_addr);
            None
        }
    }

    pub fn is_map_range_overlapped(&self, uaddr: usize, page_count: usize) -> bool {
        let end_addr = uaddr + page_count * arch::PGSIZE;

        for (&area_base, area) in &self.areas {
            let area_end = area_base + area.size();

            // Check if the range overlaps with this area
            if area_base < end_addr && area_end > uaddr {
                return true;
            }
        }

        false
    }

    pub fn map_area(&mut self, uaddr: usize, area: Box<dyn Area>) {
        assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        assert!(!self.is_map_range_overlapped(uaddr, area.page_count()), "Address range is not free");
        ktrace!("Mapping area at address: {:#x}", uaddr);
        self.areas.insert(uaddr, area);
    }

    /// Map an area at a fixed address, handling any overlapping areas
    /// 
    /// This function maps a new area at the specified address, automatically handling
    /// any existing areas that overlap with the new area's range. Overlapping areas
    /// will be split and/or removed as necessary.
    /// 
    /// # Arguments
    /// * `uaddr` - Base address where the area should be mapped (must be page-aligned)
    /// * `area` - The area to be mapped
    /// 
    /// # Returns
    /// * `Ok(())` - Success
    /// * `Err(Errno)` - Error during mapping or area splitting
    /// 
    /// # Behavior
    /// - If existing areas completely overlap with the new area, they are removed
    /// - If existing areas partially overlap, they are split to preserve non-overlapping parts
    /// - The new area is then inserted at the specified address
    /// 
    /// # Examples
    /// ```
    /// // Map a new area that may overlap with existing mappings
    /// let new_area = Box::new(AnonymousArea::new(addr, perm, page_count));
    /// manager.map_area_fixed(addr, new_area)?;
    /// ```
    #[allow(dead_code)]
    pub fn map_area_fixed(&mut self, uaddr: usize, area: Box<dyn Area>) {
        assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        
        let new_area_size = area.size();
        let new_area_end = uaddr + new_area_size;
        
        ktrace!("map_area_fixed: mapping area at {:#x}, size: {:#x}", uaddr, new_area_size);
        
        // Find all areas that overlap with the new area's range
        let mut overlapping_areas = Vec::new();
        
        for (&area_base, existing_area) in &self.areas {
            let area_end = area_base + existing_area.size();
            
            // Check if this area overlaps with the new area's range
            if area_base < new_area_end && area_end > uaddr {
                overlapping_areas.push(area_base);
                ktrace!("Found overlapping area at {:#x}-{:#x}", area_base, area_end);
            }
        }
        
        // Handle each overlapping area directly
        for overlapping_base in overlapping_areas {
            // Remove the overlapping area from the map
            let mut overlapping_area = self.areas.remove(&overlapping_base).unwrap();
            
            let overlapping_end = overlapping_base + overlapping_area.size();
            
            // ktrace!("Handling overlap: existing area {:#x}-{:#x}, new area {:#x}-{:#x}", 
            //        overlapping_base, overlapping_end, uaddr, new_area_end);
            
            // Case 1: New area completely covers the existing area
            if uaddr <= overlapping_base && new_area_end >= overlapping_end {
                ktrace!("New area completely covers existing area - removing entirely");
                // The existing area is completely covered, so we just discard it
                continue;
            }
            
            // Case 2: Need to preserve left part [overlapping_base, uaddr)
            if overlapping_base < uaddr {
                // ktrace!("Preserving left part: {:#x}-{:#x}", overlapping_base, uaddr);
                // Split at uaddr to separate left part from the rest
                let right_part = overlapping_area.split(uaddr);
                // overlapping_area now contains [overlapping_base, uaddr)
                self.areas.insert(overlapping_base, overlapping_area);
                overlapping_area = right_part; // Continue with the right part
            }
            
            // Case 3: Need to preserve right part [new_area_end, overlapping_end)
            if new_area_end < overlapping_end {
                // ktrace!("Preserving right part: {:#x}-{:#x}", new_area_end, overlapping_end);
                // Split at new_area_end to separate the part we want to keep
                let right_part = overlapping_area.split(new_area_end);
                // overlapping_area now contains the overlapped part (discard it)
                // right_part contains [new_area_end, overlapping_end) - keep this
                self.areas.insert(new_area_end, right_part);
            }
            
            // Any remaining part of overlapping_area that wasn't preserved is discarded
        }
        
        // Now we can safely insert the new area
        ktrace!("Inserting new area at address: {:#x}", uaddr);
        self.areas.insert(uaddr, area);
    }

    /// Set permissions for a specific range of pages
    /// 
    /// Changes the permissions for pages in the range [uaddr, uaddr + page_count * PGSIZE).
    /// If the range spans part of an existing area, the area will be split to accommodate
    /// the permission change.
    /// 
    /// # Arguments
    /// * `uaddr` - Start address (must be page-aligned)
    /// * `page_count` - Number of pages to modify
    /// * `perm` - New permissions to apply
    /// * `pagetable` - Page table to update
    /// 
    /// # Returns
    /// * `Ok(())` - Success
    /// * `Err(Errno)` - Error (e.g., invalid address range, no mapping found)
    pub fn set_map_area_perm(&mut self, uaddr: usize, page_count: usize, perm: MapPerm, pagetable: &RwLock<PageTable>) -> Result<(), Errno> {
        assert!(uaddr % arch::PGSIZE == 0, "uaddr must be page-aligned");
        
        if page_count == 0 {
            return Ok(());
        }
        
        let end_addr = uaddr + page_count * arch::PGSIZE;

        // Find all areas that intersect with the target range
        let mut areas_to_modify = Vec::new();
        
        for (&area_base, area) in &self.areas {
            let area_end = area_base + area.size();
            
            // Check if this area intersects with our target range
            if area_base < end_addr && area_end > uaddr {
                let intersection_start = core::cmp::max(area_base, uaddr);
                let intersection_end = core::cmp::min(area_end, end_addr);
                
                areas_to_modify.push((area_base, intersection_start, intersection_end));
                ktrace!("Found intersecting area at {:#x}-{:#x}, intersection: {:#x}-{:#x}", 
                       area_base, area_end, intersection_start, intersection_end);
            }
        }

        if areas_to_modify.is_empty() {
            ktrace!("No mapped areas found in range [{:#x}, {:#x})", uaddr, end_addr);
            return Err(Errno::ENOMEM); // No mapping found in the specified range
        }

        // Process each intersecting area
        for (area_base, intersection_start, intersection_end) in areas_to_modify {
            self.split_and_set_perm(area_base, intersection_start, intersection_end, perm, pagetable)?;
        }

        Ok(())
    }

    /// Split an area and set permissions for the specified range
    fn split_and_set_perm(&mut self, area_base: usize, range_start: usize, range_end: usize, perm: MapPerm, pagetable: &RwLock<PageTable>) -> Result<(), Errno> {
        // Remove the original area from the map
        let mut original_area = self.areas.remove(&area_base)
            .ok_or(Errno::EFAULT)?;

        let area_end = area_base + original_area.size();

        // Special case: if the range covers the entire area, just change permissions
        if range_start == area_base && range_end == area_end {
            ktrace!("Range covers entire area, just changing permissions");
            original_area.set_perm(perm, pagetable);
            self.areas.insert(area_base, original_area);
            return Ok(());
        }

        // Case 1: Range starts at area beginning but doesn't cover the whole area
        if range_start == area_base && range_end < area_end {
            ktrace!("Range starts at area beginning, splitting at end");
            let right_area = original_area.split(range_end);
            // original_area now covers [area_base, range_end)
            original_area.set_perm(perm, pagetable);
            self.areas.insert(area_base, original_area);
            self.areas.insert(range_end, right_area);
            return Ok(());
        }

        // Case 2: Range ends at area end but doesn't start at area beginning  
        if range_start > area_base && range_end == area_end {
            ktrace!("Range ends at area end, splitting at start");
            let mut right_area = original_area.split(range_start);
            // original_area now covers [area_base, range_start)
            // right_area covers [range_start, area_end)
            right_area.set_perm(perm, pagetable);
            self.areas.insert(area_base, original_area);
            self.areas.insert(range_start, right_area);
            return Ok(());
        }

        // Case 3: Range is in the middle of the area - need two splits
        if range_start > area_base && range_end < area_end {
            ktrace!("Range is in middle, need two splits");
            // First split at range_end to get the right part
            let right_area = original_area.split(range_end);
            // Now original_area covers [area_base, range_end)
            
            // Second split at range_start to get the middle part  
            let mut middle_area = original_area.split(range_start);
            // Now original_area covers [area_base, range_start)
            // middle_area covers [range_start, range_end)
            
            middle_area.set_perm(perm, pagetable);
            self.areas.insert(area_base, original_area);           // left part
            self.areas.insert(range_start, middle_area);           // middle part (new perms)
            self.areas.insert(range_end, right_area);             // right part
            return Ok(());
        }

        unreachable!();
    }

    pub fn create_user_stack(&mut self, argv: &[&str], envp: &[&str], auxv: &Auxv, pagetable: &RwLock<PageTable>) -> Result<usize, Errno> {
        assert!(self.userstack_ubase == 0, "User stack already created");
        
        let mut userstack = Box::new(UserStack::new());
        let ubase = config::USER_STACK_TOP - config::USER_STACK_PAGE_COUNT_MAX * arch::PGSIZE;
        
        let top = userstack.push_argv_envp_auxv(argv, envp, auxv, pagetable)?;

        self.map_area(ubase, userstack as Box<dyn Area>);
        self.userstack_ubase = ubase;

        Ok(top)
    }

    pub fn translate_read(&mut self, uaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize> {
        if let Some((_, area)) = self.areas.range_mut(..=uaddr).next_back() {
            area.translate_read(uaddr, pagetable)
        } else {
            None
        }
    }

    pub fn translate_write(&mut self, uaddr: usize, pagetable: &RwLock<PageTable>) -> Option<usize> {
        if let Some((_, area)) = self.areas.range_mut(..=uaddr).next_back() {
            area.translate_write(uaddr, pagetable)
        } else {
            None
        }
    }

    pub fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, pagetable: &RwLock<PageTable>) -> bool {
        if let Some((_ubase, area)) = self.areas.range_mut(..=uaddr).next_back() {
            // ktrace!("UserStack::try_to_fix_memory_fault: addr={:#x}, access_type={:?}", addr, access_type);
            area.try_to_fix_memory_fault(uaddr, access_type, pagetable)
        } else {
            false
        }
    }

    pub fn increase_userbrk(&mut self, new_ubrk: usize) -> Result<usize, Errno> {
        if new_ubrk == 0 {
            return Ok(self.userbrk.ubrk);
        }

        if new_ubrk < self.userbrk.ubrk {
            return Ok(self.userbrk.ubrk); // Do not support shrinking brk for simplicity
        }

        let new_page_count = (new_ubrk - config::USER_BRK_BASE + arch::PGSIZE - 1) / arch::PGSIZE;

        if new_page_count >= config::USER_BRK_PAGE_COUNT_MAX {
            return Err(Errno::ENOMEM);
        }

        if new_page_count > self.userbrk.page_count {
            let ubase = config::USER_BRK_BASE + self.userbrk.page_count * arch::PGSIZE;
            let new_area = Box::new(AnonymousArea::new(
                ubase, 
                MapPerm::R | MapPerm::W | MapPerm::U, 
                new_page_count - self.userbrk.page_count
            ));

            self.map_area(ubase, new_area);

            self.userbrk.page_count = new_page_count;
        }

        self.userbrk.ubrk = new_ubrk;

        Ok(self.userbrk.ubrk)
    }

    /// Print all mapped areas for debugging purposes
    /// 
    /// Outputs information about each mapped area including:
    /// - Base address and size
    /// - Area type 
    /// - Address range
    /// Print information about all mapped areas for debugging
    /// 
    /// This function outputs details about all currently mapped areas to help
    /// with debugging memory management issues.
    #[allow(dead_code)]
    pub fn print_all_areas(&self) {
        print!("=== Memory Area Manager Status ===\n");
        print!("Total areas: {}\n", self.areas.len());
        print!("User stack base: {:#x}\n", self.userstack_ubase);
        print!("User brk: {:#x} (page count: {})\n", self.userbrk.ubrk, self.userbrk.page_count);
        print!("\n");

        if self.areas.is_empty() {
            print!("No mapped areas\n");
        } else {
            print!("Mapped areas:\n");
            for (index, (&base_addr, area)) in self.areas.iter().enumerate() {
                let end_addr = base_addr + area.size();
                print!("  {}. {} [{:#x}, {:#x}) - {} bytes\n", 
                       index + 1,
                       area.type_name(),
                       base_addr, 
                       end_addr,
                       area.size());
            }
        }
        print!("=== End Memory Area Status ===\n");
    }

    /// Check if a given address range is mapped
    pub fn is_range_mapped(&self, uaddr: usize, size: usize) -> bool {
        let end_addr = uaddr + size;
        
        for (&area_base, area) in &self.areas {
            let area_end = area_base + area.size();
            
            // Check if the range overlaps with this area
            if area_base < end_addr && area_end > uaddr {
                return true;
            }
        }
        
        false
    }
}

/*
// Example usage of map_area_fixed:
//
// use crate::kernel::mm::maparea::anonymous::AnonymousArea;
// use crate::kernel::mm::Permission;
// 
// let mut manager = MapAreaManager::new();
// 
// // Example 1: Map at a clean address (no overlap)
// let area1 = Box::new(AnonymousArea::new(0x1000, Permission::ReadWrite, 4));
// manager.map_area_fixed(0x1000, area1)?;
// 
// // Example 2: Map with partial overlap (will split existing area)
// let area2 = Box::new(AnonymousArea::new(0x2000, Permission::ReadOnly, 4));
// manager.map_area_fixed(0x2000, area2)?;
// 
// // After this operation:
// // - Original area [0x1000-0x5000) becomes [0x1000-0x2000)
// // - New area [0x2000-0x6000) is mapped
// 
// // Example 3: Complete overlap replacement
// let area3 = Box::new(AnonymousArea::new(0x3000, Permission::Execute, 2));
// manager.map_area_fixed(0x3000, area3)?;
// 
// // This would further split areas as needed to accommodate the new mapping
*/
