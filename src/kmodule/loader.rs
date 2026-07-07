use alloc::vec;
use alloc::vec::Vec;
use core::mem::{align_of, size_of};
use elf::abi::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, SHN_ABS, SHN_UNDEF, SHT_NOBITS, SHT_PROGBITS, SHT_RELA, SHT_SYMTAB,
};
use elf::endian::LittleEndian;
use elf::file::Class;
use elf::parse::{ParseAt, ParseError};
use elf::relocation::Rela;
use elf::section::SectionHeader;
use elf::symbol::{Symbol, SymbolTable as ElfSymbolTable};
use elf::{ElfBytes, abi};
use fixedstr::tstr;

use crate::arch;
use crate::fs::file::FileOps;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::{ContiguousPhysPageFrame, MapPerm};
use crate::kmodule::{KModuleRelocationAction, KModuleRelocationValue, MAX_KMODULE_IMAGE_SIZE, exports};

const MODULE_INFO_SECTION: &str = "kernelx.module.info";
const MODULE_NAME_LEN: usize = 256;

type KModuleElf<'a> = ElfBytes<'a, LittleEndian>;
type KModuleInit = extern "C" fn() -> i32;
type KModuleExit = extern "C" fn();

#[repr(C)]
#[derive(Clone, Copy)]
struct KModuleInfo {
    module_name: [u8; MODULE_NAME_LEN],
    init: *const (),
    exit: *const (),
}

struct ModuleInfo {
    name: tstr<MODULE_NAME_LEN>,
    init: usize,
    exit: usize,
}

impl ModuleInfo {
    fn from_raw(info: KModuleInfo) -> SysResult<Self> {
        let name = module_name(&info.module_name)?;
        let init = info.init as usize;
        let exit = info.exit as usize;
        if init == 0 || exit == 0 {
            return Err(Errno::ENOEXEC);
        }
        Ok(Self { name, init, exit })
    }
}

#[derive(Clone, Copy)]
struct LoadedSection {
    addr: usize,
    offset: usize,
    perm: MapPerm,
    size: usize,
}

struct ModulePages {
    pages: ContiguousPhysPageFrame,
}

impl ModulePages {
    fn new(size: usize) -> Self {
        let page_count = arch::page_count(size);
        let pages = ContiguousPhysPageFrame::alloc(page_count);
        pages.slice().fill(0);
        Self { pages }
    }

    fn base(&self) -> usize {
        self.pages.get_page()
    }

    fn apply_section_permissions(&self, sections: &[Option<LoadedSection>], got_section: Option<LoadedSection>) {
        let size = self.pages.size();
        let base = self.pages.get_page();
        arch::map_kernel_addr(base, arch::kaddr_to_paddr(base), size, MapPerm::R);

        for section in sections.iter().flatten() {
            self.apply_section_permission(section);
        }
        if let Some(section) = got_section {
            self.apply_section_permission(&section);
        }
        arch::flush_kmodule_icache();
    }

    fn apply_section_permission(&self, section: &LoadedSection) {
        let size = arch::page_count(section.size) * arch::PGSIZE;
        arch::map_kernel_addr(section.addr, arch::kaddr_to_paddr(section.addr), size, section.perm);
    }
}

impl Drop for ModulePages {
    fn drop(&mut self) {
        arch::map_kernel_addr(
            self.pages.get_page(),
            arch::kaddr_to_paddr(self.pages.get_page()),
            self.pages.size(),
            MapPerm::RW,
        );
    }
}

pub(super) fn load_file(file: &dyn FileOps) -> SysResult<LinkedModule> {
    let image = read_module_image(file)?;
    load_image(&image)
}

pub(super) fn load_image(image: &[u8]) -> SysResult<LinkedModule> {
    validate_image_size(image.len())?;
    LinkedModule::load(image)
}

fn read_module_image(file: &dyn FileOps) -> SysResult<Vec<u8>> {
    if !file.readable() {
        return Err(Errno::EBADF);
    }

    let stat = file.fstat()?;
    if stat.st_size <= 0 {
        return Err(Errno::ENOEXEC);
    }

    let size = stat.st_size as usize;
    validate_image_size(size)?;

    let mut image = vec![0; size];
    let mut read_len = 0;
    while read_len < size {
        let n = file.pread(&mut image[read_len..], read_len)?;
        if n == 0 {
            return Err(Errno::EIO);
        }
        read_len += n;
    }
    Ok(image)
}

fn validate_image_size(size: usize) -> SysResult<()> {
    if size == 0 {
        return Err(Errno::ENOEXEC);
    }
    if size > MAX_KMODULE_IMAGE_SIZE {
        return Err(Errno::EFBIG);
    }
    Ok(())
}

pub(super) struct LinkedModule {
    pages: ModulePages,
    info: ModuleInfo,
}

impl LinkedModule {
    fn load(image: &[u8]) -> SysResult<Self> {
        Loader::load(image)
    }

    pub(super) fn name(&self) -> tstr<MODULE_NAME_LEN> {
        self.info.name
    }

    pub(super) fn call_init(&self) -> SysResult<i32> {
        let _keep_pages_alive = &self.pages;
        // SAFETY: init is parsed from the relocated kernelx.module.info record,
        // and the kmodule ABI requires it to have the extern "C" fn() -> i32
        // signature.
        let init: KModuleInit = unsafe { core::mem::transmute(self.info.init) };
        Ok(init())
    }

    pub(super) fn call_exit(&self) {
        let _keep_pages_alive = &self.pages;
        // SAFETY: exit is parsed from the relocated kernelx.module.info record,
        // and the kmodule ABI requires it to have the extern "C" fn() signature.
        let exit: KModuleExit = unsafe { core::mem::transmute(self.info.exit) };
        exit();
    }
}

struct SymbolTable {
    addrs: Vec<Option<usize>>,
}

impl SymbolTable {
    fn new(addrs: Vec<Option<usize>>) -> Self {
        Self { addrs }
    }

    fn addr(&self, index: usize) -> SysResult<usize> {
        self.addrs.get(index).copied().flatten().ok_or(Errno::ENOEXEC)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct GotKey {
    symtab_index: usize,
    symbol_index: usize,
    addend: i64,
}

impl GotKey {
    fn from_rela(symtab_index: usize, rela: &Rela) -> Self {
        Self {
            symtab_index,
            symbol_index: rela.r_sym as usize,
            addend: rela.r_addend,
        }
    }
}

#[derive(Clone, Copy)]
struct GotEntry {
    key: GotKey,
    addr: Option<usize>,
}

struct GotTable {
    entries: Vec<GotEntry>,
    section: Option<LoadedSection>,
}

impl GotTable {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            section: None,
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.section = None;
    }

    fn insert(&mut self, symtab_index: usize, rela: &Rela) {
        let key = GotKey::from_rela(symtab_index, rela);
        if self.entries.iter().any(|entry| entry.key == key) {
            return;
        }
        self.entries.push(GotEntry { key, addr: None });
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn size(&self) -> SysResult<usize> {
        self.entries.len().checked_mul(size_of::<usize>()).ok_or(Errno::ENOEXEC)
    }

    fn loaded_section(&self) -> Option<LoadedSection> {
        self.section
    }

    fn load_at(&mut self, base: usize, offset: usize) -> SysResult<()> {
        let size = self.size()?;
        if size == 0 {
            self.section = None;
            return Ok(());
        }

        let addr = base.checked_add(offset).ok_or(Errno::ENOEXEC)?;
        self.section = Some(LoadedSection {
            addr,
            offset,
            perm: MapPerm::R,
            size,
        });

        for (index, entry) in self.entries.iter_mut().enumerate() {
            let entry_offset = index.checked_mul(size_of::<usize>()).ok_or(Errno::ENOEXEC)?;
            entry.addr = Some(addr.checked_add(entry_offset).ok_or(Errno::ENOEXEC)?);
        }
        Ok(())
    }

    fn entry_addr(&self, symtab_index: usize, rela: &Rela) -> SysResult<usize> {
        let key = GotKey::from_rela(symtab_index, rela);
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .and_then(|entry| entry.addr)
            .ok_or(Errno::ENOEXEC)
    }
}

struct Loader<'a> {
    image: &'a [u8],
    file: KModuleElf<'a>,
    shdrs: Vec<SectionHeader>,
    sections: Vec<Option<LoadedSection>>,
    got: GotTable,

    symbol_tables_by_section: Vec<Option<SymbolTable>>,
}

impl<'a> Loader<'a> {
    fn load(image: &'a [u8]) -> SysResult<LinkedModule> {
        let mut loader = Self::new(image)?;
        loader.collect_got_entries()?;
        let pages = loader.load_alloc_sections()?;
        loader.link()?;
        let info = loader.module_info()?;
        pages.apply_section_permissions(&loader.sections, loader.got.loaded_section());

        Ok(LinkedModule { pages, info })
    }

    fn new(image: &'a [u8]) -> SysResult<Self> {
        let file = errno_to_kernel(ElfBytes::<LittleEndian>::minimal_parse(image))?;
        if !(file.ehdr.class == Class::ELF64
            && file.ehdr.e_type == abi::ET_REL
            && file.ehdr.e_machine == arch::elf_native_machine())
        {
            return Err(Errno::ENOEXEC);
        }

        let shdrs = Self::read_section_headers(&file)?;
        let section_count = shdrs.len();
        Ok(Self {
            image,
            file,
            shdrs,
            sections: vec![None; section_count],
            got: GotTable::new(),
            symbol_tables_by_section: (0..section_count).map(|_| None).collect(),
        })
    }

    fn read_section_headers(file: &KModuleElf<'_>) -> SysResult<Vec<SectionHeader>> {
        let table = file.section_headers().ok_or(Errno::ENOEXEC)?;
        if table.is_empty() {
            return Err(Errno::ENOEXEC);
        }

        let count = table.len();
        let mut shdrs = Vec::with_capacity(count);
        for index in 0..count {
            shdrs.push(errno_to_kernel(table.get(index))?);
        }
        Ok(shdrs)
    }

    fn module_info(&self) -> SysResult<ModuleInfo> {
        let (_, strtab) = errno_to_kernel(self.file.section_headers_with_strtab())?;
        let strtab = strtab.ok_or(Errno::ENOEXEC)?;
        let mut info = None;

        for (index, shdr) in self.shdrs.iter().enumerate() {
            let name = errno_to_kernel(strtab.get(shdr.sh_name as usize))?;
            if name != MODULE_INFO_SECTION {
                continue;
            }
            if info.is_some() {
                crate::kwarn!("kmodule: duplicate {} section", MODULE_INFO_SECTION);
                return Err(Errno::ENOEXEC);
            }

            let Some(section) = self.sections.get(index).and_then(|section| *section) else {
                crate::kwarn!("kmodule: {} section is not loaded", MODULE_INFO_SECTION);
                return Err(Errno::ENOEXEC);
            };
            if section.size != size_of::<KModuleInfo>() {
                crate::kwarn!(
                    "kmodule: invalid {} size {}, expected {}",
                    MODULE_INFO_SECTION,
                    section.size,
                    size_of::<KModuleInfo>()
                );
                return Err(Errno::ENOEXEC);
            }

            // SAFETY: section.addr points to the loaded kernelx.module.info
            // section, and its size was checked against KModuleInfo above.
            let raw = unsafe { core::ptr::read_unaligned(section.addr as *const KModuleInfo) };
            info = Some(ModuleInfo::from_raw(raw)?);
        }

        info.ok_or(Errno::ENOEXEC)
    }

    fn collect_got_entries(&mut self) -> SysResult<()> {
        self.got.clear();

        for rela_index in 0..self.shdrs.len() {
            let (symtab_index, relas) = {
                let rela_shdr = &self.shdrs[rela_index];
                if rela_shdr.sh_type != SHT_RELA {
                    continue;
                }
                (rela_shdr.sh_link as usize, self.read_relas(rela_shdr)?)
            };

            for rela in &relas {
                let action = match arch::kmodule_relocation_action(rela.r_type) {
                    Ok(action) => action,
                    Err(_) => continue,
                };
                if matches!(action, KModuleRelocationAction::ResolveGotEntry) {
                    self.got.insert(symtab_index, rela);
                }
            }
        }
        Ok(())
    }

    fn load_alloc_sections(&mut self) -> SysResult<ModulePages> {
        let mut total_size = 0;
        for section in &mut self.sections {
            *section = None;
        }

        for (index, shdr) in self.shdrs.iter().enumerate() {
            if shdr.sh_flags & SHF_ALLOC as u64 == 0 || shdr.sh_size == 0 {
                continue;
            }

            let section_align = to_usize(shdr.sh_addralign.max(1))?;
            if !section_align.is_power_of_two() {
                return Err(Errno::ENOEXEC);
            }
            let alloc_align = section_align.max(arch::PGSIZE);
            let offset = align_up(total_size, alloc_align);
            let size = to_usize(shdr.sh_size)?;
            total_size = offset.checked_add(size).ok_or(Errno::ENOEXEC)?;

            let mut perm = MapPerm::R;
            if shdr.sh_flags & SHF_WRITE as u64 != 0 {
                perm |= MapPerm::W;
            }
            if shdr.sh_flags & SHF_EXECINSTR as u64 != 0 {
                perm |= MapPerm::X;
            }
            self.sections[index] = Some(LoadedSection {
                addr: 0,
                offset,
                perm,
                size,
            });
        }

        let got_offset = if self.got.is_empty() {
            None
        } else {
            let offset = align_up(total_size, arch::PGSIZE.max(align_of::<usize>()));
            let size = self.got.size()?;
            total_size = offset.checked_add(size).ok_or(Errno::ENOEXEC)?;
            Some(offset)
        };

        if total_size == 0 {
            return Err(Errno::ENOEXEC);
        }

        let pages = ModulePages::new(total_size);
        if let Some(offset) = got_offset {
            self.got.load_at(pages.base(), offset)?;
        }

        for (index, section) in self.sections.iter_mut().enumerate() {
            let Some(section) = section.as_mut() else {
                continue;
            };
            section.addr = pages.base() + section.offset;

            let shdr = &self.shdrs[index];
            match shdr.sh_type {
                SHT_PROGBITS => {
                    let src = checked_slice(self.image, to_usize(shdr.sh_offset)?, section.size)?;
                    // SAFETY: src was bounds-checked against the ELF image, and
                    // section.addr points to the loaded section range reserved in
                    // ModulePages with at least section.size writable bytes.
                    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), section.addr as *mut u8, section.size) };
                }
                SHT_NOBITS => {}
                _ => return Err(Errno::ENOEXEC),
            }
        }

        Ok(pages)
    }

    fn link(&mut self) -> SysResult<()> {
        if let Err(err) = self.resolve_symbols() {
            crate::kwarn!("kmodule: resolve symbols failed: {:?}", err);
            return Err(err);
        }

        if let Err(err) = self.write_got_entries() {
            crate::kwarn!("kmodule: write GOT entries failed: {:?}", err);
            return Err(err);
        }

        if let Err(err) = self.apply_relocations() {
            crate::kwarn!("kmodule: apply relocations failed: {:?}", err);
            return Err(err);
        }
        Ok(())
    }

    fn resolve_symbols(&mut self) -> SysResult<()> {
        for symbols in &mut self.symbol_tables_by_section {
            *symbols = None;
        }

        for symtab_index in 0..self.shdrs.len() {
            let symtab = &self.shdrs[symtab_index];
            if symtab.sh_type != SHT_SYMTAB {
                continue;
            }

            let count = symtab_entry_count(symtab)?;
            let mut symbol_addrs = Vec::with_capacity(count);
            for index in 0..count {
                let sym = self.read_symbol(symtab, index)?;
                symbol_addrs.push(self.resolve_symbol_addr(symtab, &sym)?);
            }
            self.symbol_tables_by_section[symtab_index] = Some(SymbolTable::new(symbol_addrs));
        }
        Ok(())
    }

    fn write_got_entries(&self) -> SysResult<()> {
        for entry in &self.got.entries {
            let addr = entry.addr.ok_or(Errno::ENOEXEC)?;
            let value =
                self.relocation_addr_for_symbol(entry.key.symtab_index, entry.key.symbol_index, entry.key.addend)?;
            // SAFETY: GOT entry addresses are assigned inside the module page
            // allocation by GotTable::load_at, and each slot is exactly one
            // usize reserved for a resolved symbol address.
            unsafe { (addr as *mut usize).write(value) };
        }
        Ok(())
    }

    fn apply_relocations(&self) -> SysResult<()> {
        for rela_shdr in &self.shdrs {
            if rela_shdr.sh_type != SHT_RELA {
                continue;
            }

            let target_index = rela_shdr.sh_info as usize;
            let symtab_index = rela_shdr.sh_link as usize;
            let Some(target) = self.sections.get(target_index).and_then(|section| *section) else {
                crate::kwarn!("kmodule: relocation target section {} is not loaded", target_index);
                return Err(Errno::ENOEXEC);
            };
            errno_to_kernel(Rela::validate_entsize(
                self.file.ehdr.class,
                to_usize(rela_shdr.sh_entsize)?,
            ))?;

            let relas = self.read_relas(rela_shdr)?;
            let reference_targets = match self.collect_relocation_reference_targets(symtab_index, target, &relas) {
                Ok(targets) => targets,
                Err(err) => {
                    crate::kwarn!(
                        "kmodule: failed to collect relocation reference targets for section {}: {:?}",
                        target_index,
                        err
                    );
                    return Err(err);
                }
            };

            for rela in relas {
                let offset = rela.r_offset as usize;
                if offset >= target.size {
                    crate::kwarn!(
                        "kmodule: relocation offset out of range section={} offset={:#x} size={:#x} type={} sym={}",
                        target_index,
                        offset,
                        target.size,
                        rela.r_type,
                        rela.r_sym
                    );
                    return Err(Errno::ENOEXEC);
                }
                let place_addr = target.addr.checked_add(offset).ok_or(Errno::ENOEXEC)?;

                let action = match arch::kmodule_relocation_action(rela.r_type) {
                    Ok(action) => action,
                    Err(err) => {
                        crate::kwarn!(
                            "kmodule: unsupported relocation type={} sym={} offset={:#x}: {:?}",
                            rela.r_type,
                            rela.r_sym,
                            offset,
                            err
                        );
                        return Err(err);
                    }
                };
                let value = match action {
                    KModuleRelocationAction::None => None,
                    KModuleRelocationAction::ResolveSymbol
                    | KModuleRelocationAction::ResolveGotEntry
                    | KModuleRelocationAction::ResolveSymbolAndRecordReferenceTarget => {
                        match self.direct_relocation_value(symtab_index, place_addr, &rela, action) {
                            Ok(value) => Some(value),
                            Err(err) => {
                                crate::kwarn!(
                                    "kmodule: failed to resolve relocation value type={} sym={} offset={:#x}: {:?}",
                                    rela.r_type,
                                    rela.r_sym,
                                    offset,
                                    err
                                );
                                return Err(err);
                            }
                        }
                    }
                    KModuleRelocationAction::ResolveReferencedRelocation => {
                        let reference_target_place = self.symbol_addr(symtab_index, rela.r_sym as usize)?;
                        let Some(reference_target) = reference_targets
                            .iter()
                            .find(|target| target.base == reference_target_place)
                        else {
                            crate::kwarn!(
                                "kmodule: missing referenced relocation target type={} sym={} offset={:#x} reference_place={:#x}",
                                rela.r_type,
                                rela.r_sym,
                                offset,
                                reference_target_place
                            );
                            return Err(Errno::ENOEXEC);
                        };
                        Some(*reference_target)
                    }
                };
                // SAFETY: place_addr is inside target's loaded section and
                // target.size - offset bounds the slice to that section.
                let place = unsafe { core::slice::from_raw_parts_mut(place_addr as *mut u8, target.size - offset) };
                if let Err(err) = arch::apply_kmodule_relocation(rela.r_type, place, value) {
                    crate::kwarn!(
                        "kmodule: failed to apply relocation type={} sym={} offset={:#x} place={:#x}: {:?}",
                        rela.r_type,
                        rela.r_sym,
                        offset,
                        place_addr,
                        err
                    );
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    fn collect_relocation_reference_targets(
        &self,
        symtab_index: usize,
        target: LoadedSection,
        relas: &[Rela],
    ) -> SysResult<Vec<KModuleRelocationValue>> {
        let mut reference_targets = Vec::new();
        for rela in relas {
            let action = match arch::kmodule_relocation_action(rela.r_type) {
                Ok(action) => action,
                Err(_) => continue,
            };
            if !matches!(
                action,
                KModuleRelocationAction::ResolveSymbolAndRecordReferenceTarget
                    | KModuleRelocationAction::ResolveGotEntry
            ) {
                continue;
            }
            let place = target
                .addr
                .checked_add(to_usize(rela.r_offset)?)
                .ok_or(Errno::ENOEXEC)?;
            match self.direct_relocation_value(symtab_index, place, rela, action) {
                Ok(value) => reference_targets.push(value),
                Err(err) => {
                    crate::kwarn!(
                        "kmodule: failed to resolve reference target type={} sym={} offset={:#x}: {:?}",
                        rela.r_type,
                        rela.r_sym,
                        rela.r_offset,
                        err
                    );
                    return Err(err);
                }
            }
        }
        Ok(reference_targets)
    }

    fn read_relas(&self, rela_shdr: &SectionHeader) -> SysResult<Vec<Rela>> {
        let entsize = to_usize(rela_shdr.sh_entsize)?;
        let size = to_usize(rela_shdr.sh_size)?;
        if entsize == 0 || size % entsize != 0 {
            return Err(Errno::ENOEXEC);
        }

        let count = size / entsize;
        let mut relas = Vec::with_capacity(count);
        for rela in errno_to_kernel(self.file.section_data_as_relas(rela_shdr))? {
            relas.push(rela);
        }
        if relas.len() != count {
            return Err(Errno::ENOEXEC);
        }
        Ok(relas)
    }

    fn symbol_addr(&self, symtab_index: usize, symbol_index: usize) -> SysResult<usize> {
        self.symbol_tables_by_section
            .get(symtab_index)
            .and_then(Option::as_ref)
            .ok_or(Errno::ENOEXEC)?
            .addr(symbol_index)
    }

    fn relocation_addr(&self, symtab_index: usize, rela: &Rela) -> SysResult<usize> {
        self.relocation_addr_for_symbol(symtab_index, rela.r_sym as usize, rela.r_addend)
    }

    fn relocation_addr_for_symbol(&self, symtab_index: usize, symbol_index: usize, addend: i64) -> SysResult<usize> {
        let base = self.symbol_addr(symtab_index, symbol_index)?;
        if addend >= 0 {
            base.checked_add(addend as usize).ok_or(Errno::ENOEXEC)
        } else {
            let addend = addend.checked_neg().ok_or(Errno::ENOEXEC)? as usize;
            base.checked_sub(addend).ok_or(Errno::ENOEXEC)
        }
    }

    fn direct_relocation_value(
        &self,
        symtab_index: usize,
        place: usize,
        rela: &Rela,
        action: KModuleRelocationAction,
    ) -> SysResult<KModuleRelocationValue> {
        let value = match action {
            KModuleRelocationAction::ResolveSymbol | KModuleRelocationAction::ResolveSymbolAndRecordReferenceTarget => {
                self.relocation_addr(symtab_index, rela)?
            }
            KModuleRelocationAction::ResolveGotEntry => self.got.entry_addr(symtab_index, rela)?,
            KModuleRelocationAction::None | KModuleRelocationAction::ResolveReferencedRelocation => {
                return Err(Errno::ENOEXEC);
            }
        };
        Ok(KModuleRelocationValue { base: place, value })
    }

    fn resolve_symbol_addr(&self, symtab: &SectionHeader, sym: &Symbol) -> SysResult<Option<usize>> {
        if sym.st_shndx == SHN_ABS {
            return Ok(Some(to_usize(sym.st_value)?));
        }
        if sym.st_shndx != SHN_UNDEF {
            // The symbol is defined in a section of this module, so we can compute its address directly.
            return self.symbol_defined_addr(sym);
        }

        let name = self.symbol_name(symtab, sym)?;
        if name.is_empty() {
            return Ok(Some(0));
        }
        if let Some(addr) = exports::resolve(name) {
            return Ok(Some(addr));
        }
        crate::kwarn!("kmodule: unresolved external symbol `{}`", name);
        Err(Errno::ENOEXEC)
    }

    fn symbol_defined_addr(&self, sym: &Symbol) -> SysResult<Option<usize>> {
        let Some(section) = self.sections.get(sym.st_shndx as usize).map(Option::as_ref).flatten() else {
            return Ok(None);
        };
        Ok(Some(
            section
                .addr
                .checked_add(to_usize(sym.st_value)?)
                .ok_or(Errno::ENOEXEC)?,
        ))
    }

    fn read_symbol(&self, symtab: &SectionHeader, index: usize) -> SysResult<Symbol> {
        errno_to_kernel(self.symbol_table(symtab)?.get(index))
    }

    fn symbol_name(&self, symtab: &SectionHeader, sym: &Symbol) -> SysResult<&str> {
        let strtab = self.shdrs.get(symtab.sh_link as usize).ok_or(Errno::ENOEXEC)?;
        let strtab = errno_to_kernel(self.file.section_data_as_strtab(strtab))?;
        errno_to_kernel(strtab.get(sym.st_name as usize))
    }

    fn symbol_table(&self, symtab: &SectionHeader) -> SysResult<ElfSymbolTable<'_, LittleEndian>> {
        errno_to_kernel(Symbol::validate_entsize(
            self.file.ehdr.class,
            to_usize(symtab.sh_entsize)?,
        ))?;
        let (bytes, _) = errno_to_kernel(self.file.section_data(symtab))?;
        Ok(ElfSymbolTable::new(
            self.file.ehdr.endianness,
            self.file.ehdr.class,
            bytes,
        ))
    }
}

fn symtab_entry_count(symtab: &SectionHeader) -> SysResult<usize> {
    let entsize = to_usize(symtab.sh_entsize)?;
    let size = to_usize(symtab.sh_size)?;
    if entsize == 0 || size % entsize != 0 {
        return Err(Errno::ENOEXEC);
    }
    Ok(size / entsize)
}

fn errno_to_kernel<T>(result: Result<T, ParseError>) -> SysResult<T> {
    result.map_err(|_| Errno::ENOEXEC)
}

fn to_usize(value: u64) -> SysResult<usize> {
    usize::try_from(value).map_err(|_| Errno::ENOEXEC)
}

fn checked_slice(image: &[u8], offset: usize, size: usize) -> SysResult<&[u8]> {
    let end = offset.checked_add(size).ok_or(Errno::ENOEXEC)?;
    image.get(offset..end).ok_or(Errno::ENOEXEC)
}

fn module_name(name: &[u8; MODULE_NAME_LEN]) -> SysResult<tstr<MODULE_NAME_LEN>> {
    let len = name.iter().position(|&ch| ch == 0).ok_or(Errno::ENOEXEC)?;
    if len == 0 {
        return Err(Errno::ENOEXEC);
    }
    let name = core::str::from_utf8(&name[..len]).map_err(|_| Errno::ENOEXEC)?;
    Ok(tstr::from(name))
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
