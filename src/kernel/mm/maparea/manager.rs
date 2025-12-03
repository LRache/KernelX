use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::anonymous::AnonymousArea;
use crate::kernel::mm::{AddrSpace, MapPerm, MemAccessType};
use crate::arch::{self, PageTable};
use crate::{ktrace, print};

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
                // ktrace!("Found mmap address {:#x} for {} pages (gap before area at {:#x})", 
                //        candidate_addr, page_count, area_base);
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
        if page_count == 0 {
            return false;
        }

        let end_addr = uaddr.saturating_add(page_count * arch::PGSIZE);

        // Previous area (if any) might extend into the new range.
        if let Some((area_base, area)) = self.areas.range(..=uaddr).next_back() {
            let area_end = area_base.saturating_add(area.size());
            if uaddr < area_end {
                return true;
            }
        }

        // Next area is the only other candidate because areas are non-overlapping and ordered.
        if let Some((area_base, _)) = self.areas.range(uaddr..).next() {
            if *area_base < end_addr {
                return true;
            }
        }

        false
    }

    pub fn map_area(&mut self, uaddr: usize, area: Box<dyn Area>) {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(!self.is_map_range_overlapped(uaddr, area.page_count()), "Address range is not free");
        self.areas.insert(uaddr, area);
    }

    fn find_overlapped_areas(&self, start: usize, end: usize) -> Vec<usize> {
        let mut overlapped_areas = Vec::new();
        let iter_start = self.areas.range(..=start).next().map(|(k, _)| *k).unwrap_or(0);

        for (&area_base, area) in self.areas.range(iter_start..) {
            if end <= area_base {
                break;
            }

            if area_base + area.size() <= start {
                continue;
            }
            
            overlapped_areas.push(area_base);
        }

        overlapped_areas
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
    pub fn map_area_fixed(&mut self, uaddr: usize, area: Box<dyn Area>, pagetable: &RwLock<PageTable>) {
        assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        
        let new_area_end = uaddr + area.size();
        
        // Find all areas that overlap with the new area's range
        // let mut overlapping_areas = Vec::new();

        for overlapping_base in self.find_overlapped_areas(uaddr, new_area_end) {
            let mut middle = self.areas.remove(&overlapping_base).unwrap();
            let overlapping_end = overlapping_base + middle.size();

            // KEEP Left part [overlapping_base, uaddr)
            if overlapping_base < uaddr {
                let left;
                (left, middle) = middle.split(uaddr);
                self.areas.insert(overlapping_base, left);
            }

            // KEEP Right part [new_area_end, overlapping_end)
            if new_area_end < overlapping_end {
                let right;
                (middle, right) = middle.split(new_area_end);
                self.areas.insert(new_area_end, right);
            }

            // UNMAP Middle part [max(overlapping_base, uaddr), min(overlapping_end, new_area_end))
            middle.unmap(pagetable);
        }
        
        // Now we can safely insert the new area
        self.areas.insert(uaddr, area);
    }

    pub fn unmap_area(&mut self, uaddr: usize, page_count: usize, pagetable: &RwLock<PageTable>) -> SysResult<()> {
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr should be page-aligned");
        debug_assert!(page_count > 0, "page_count should be greater than 0");

        let uaddr_end = uaddr + page_count * arch::PGSIZE;

        // Process each intersecting area
        for area_base in self.find_overlapped_areas(uaddr, uaddr_end) {
            // Remove the area from the map
            let mut middle = self.areas.remove(&area_base).unwrap();
            let area_end = area_base + middle.size();

            // KEEP Left part [area_base, uaddr)
            if area_base < uaddr {
                let left;
                (left, middle) = middle.split(uaddr);
                self.areas.insert(area_base, left);
            }

            // KEEP Right part [uaddr_end, area_end)
            if uaddr_end < area_end {
                let right;
                (middle, right) = middle.split(uaddr_end);
                self.areas.insert(uaddr_end, right);
            }

            // UNMAP Middle part [max(area_base, uaddr), min(area_end, uaddr_end))
            middle.unmap(pagetable);
        }

        Ok(())
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
        debug_assert!(uaddr % arch::PGSIZE == 0, "uaddr must be page-aligned");
        
        if page_count == 0 {
            return Ok(());
        }
        
        let uaddr_end = uaddr + page_count * arch::PGSIZE;

        for overlapped_base in self.find_overlapped_areas(uaddr, uaddr_end) {
            let mut middle = self.areas.remove(&overlapped_base).unwrap();
            let overlapped_end = overlapped_base + middle.size();

            if overlapped_base < uaddr {
                let left;
                (left, middle) = middle.split(uaddr);
                self.areas.insert(overlapped_base, left);
            }

            if uaddr_end < overlapped_end {
                let right;
                (middle, right) = middle.split(uaddr_end);
                self.areas.insert(uaddr_end, right);
            }

            middle.set_perm(perm, pagetable);
            self.areas.insert(middle.ubase(), middle);
        }

        Ok(())
    }

    /// Split an area and set permissions for the specified range
    // fn split_and_set_perm(&mut self, area_base: usize, range_start: usize, range_end: usize, perm: MapPerm, pagetable: &RwLock<PageTable>) -> Result<(), Errno> {
    //     // Remove the original area from the map
    //     let mut original_area = self.areas.remove(&area_base)
    //         .ok_or(Errno::EFAULT)?;

    //     let area_end = area_base + original_area.size();

    //     // Special case: if the range covers the entire area, just change permissions
    //     if range_start == area_base && range_end == area_end {
    //         ktrace!("Range covers entire area, just changing permissions");
    //         original_area.set_perm(perm, pagetable);
    //         self.areas.insert(area_base, original_area);
    //         return Ok(());
    //     }

    //     // Case 1: Range starts at area beginning but doesn't cover the whole area
    //     if range_start == area_base && range_end < area_end {
    //         ktrace!("Range starts at area beginning, splitting at end");
    //         let right_area = original_area.split(range_end);
    //         // original_area now covers [area_base, range_end)
    //         original_area.set_perm(perm, pagetable);
    //         self.areas.insert(area_base, original_area);
    //         self.areas.insert(range_end, right_area);
    //         return Ok(());
    //     }

    //     // Case 2: Range ends at area end but doesn't start at area beginning  
    //     if range_start > area_base && range_end == area_end {
    //         ktrace!("Range ends at area end, splitting at start");
    //         let mut right_area = original_area.split(range_start);
    //         // original_area now covers [area_base, range_start)
    //         // right_area covers [range_start, area_end)
    //         right_area.set_perm(perm, pagetable);
    //         self.areas.insert(area_base, original_area);
    //         self.areas.insert(range_start, right_area);
    //         return Ok(());
    //     }

    //     // Case 3: Range is in the middle of the area - need two splits
    //     if range_start > area_base && range_end < area_end {
    //         ktrace!("Range is in middle, need two splits");
    //         // First split at range_end to get the right part
    //         let right_area = original_area.split(range_end);
    //         // Now original_area covers [area_base, range_end)
            
    //         // Second split at range_start to get the middle part  
    //         let mut middle_area = original_area.split(range_start);
    //         // Now original_area covers [area_base, range_start)
    //         // middle_area covers [range_start, range_end)
            
    //         middle_area.set_perm(perm, pagetable);
    //         self.areas.insert(area_base, original_area);           // left part
    //         self.areas.insert(range_start, middle_area);           // middle part (new perms)
    //         self.areas.insert(range_end, right_area);             // right part
    //         return Ok(());
    //     }

    //     unreachable!();
    // }

    pub fn create_user_stack(&mut self, argv: &[&str], envp: &[&str], auxv: &Auxv, addrspace: &AddrSpace) -> Result<usize, Errno> {
        assert!(self.userstack_ubase == 0, "User stack already created");
        
        let mut userstack = Box::new(UserStack::new());
        let ubase = config::USER_STACK_TOP - config::USER_STACK_PAGE_COUNT_MAX * arch::PGSIZE;
        
        let top = userstack.push_argv_envp_auxv(argv, envp, auxv, addrspace)?;

        self.map_area(ubase, userstack as Box<dyn Area>);
        self.userstack_ubase = ubase;

        Ok(top)
    }

    pub fn translate_read(&mut self, uaddr: usize, addrspace: &Arc<AddrSpace>) -> Option<usize> {
        if let Some((_, area)) = self.areas.range_mut(..=uaddr).next_back() {
            area.translate_read(uaddr, addrspace)
        } else {
            None
        }
    }

    pub fn translate_write(&mut self, uaddr: usize, addrspace: &Arc<AddrSpace>) -> Option<usize> {
        if let Some((_, area)) = self.areas.range_mut(..=uaddr).next_back() {
            area.translate_write(uaddr, addrspace)
        } else {
            None
        }
    }

    pub fn try_to_fix_memory_fault(&mut self, uaddr: usize, access_type: MemAccessType, addrspace: &Arc<AddrSpace>) -> bool {
        if let Some((_ubase, area)) = self.areas.range_mut(..=uaddr).next_back() {
            if !access_type.match_perm(area.perm()) {
                return false;
            }
            // kinfo!("Trying to fix memory fault at address {:#x} with access type {:?}", uaddr, access_type);
            area.try_to_fix_memory_fault(uaddr, access_type, addrspace)
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

        if new_page_count > self.userbrk.page_count {
            let ubase = config::USER_BRK_BASE + self.userbrk.page_count * arch::PGSIZE;
            let new_area = Box::new(AnonymousArea::new(
                ubase, 
                MapPerm::R | MapPerm::W | MapPerm::U, 
                new_page_count - self.userbrk.page_count,
                false
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
