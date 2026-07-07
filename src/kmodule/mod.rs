pub mod exports;
mod loader;
pub mod wrapper;

use alloc::vec::Vec;
use fixedstr::tstr;

use crate::fs::file::FileOps;
use crate::kernel::errno::{Errno, SysResult};
use crate::klib::SpinLock;

pub(crate) const MAX_KMODULE_IMAGE_SIZE: usize = 1024 * 1024;
const MODULE_NAME_LEN: usize = 256;

static MODULES: SpinLock<Vec<LoadedModule>> = SpinLock::new(Vec::new(), "static::KMODULES");

struct LoadedModule {
    name: tstr<MODULE_NAME_LEN>,
    module: loader::LinkedModule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KModuleRelocationAction {
    None,
    ResolveSymbol,
    /// Resolve to the module-local GOT entry that stores this symbol's address.
    /// GOT relocation places are also recorded as reference targets for paired
    /// low relocations on architectures such as RISC-V.
    ResolveGotEntry,
    /// Resolve the symbol and record this relocation place as a target that
    /// another relocation symbol may reference.
    ResolveSymbolAndRecordReferenceTarget,
    /// Resolve through a previously recorded relocation place referenced by
    /// this relocation's symbol.
    ResolveReferencedRelocation,
}

#[derive(Clone, Copy, Debug)]
pub struct KModuleRelocationValue {
    pub base: usize,
    pub value: usize,
}

pub fn load(image: &[u8]) -> SysResult<i32> {
    load_module(loader::load_image(image)?)
}

pub fn load_file(file: &dyn FileOps) -> SysResult<i32> {
    load_module(loader::load_file(file)?)
}

pub fn unload(name: tstr<MODULE_NAME_LEN>) -> SysResult<()> {
    let module = {
        let mut modules = MODULES.lock();
        let index = modules
            .iter()
            .position(|module| module.name == name)
            .ok_or(Errno::ENOENT)?;
        modules.remove(index).module
    };

    module.call_exit();
    Ok(())
}

fn load_module(module: loader::LinkedModule) -> SysResult<i32> {
    let name = module.name();
    if MODULES.lock().iter().any(|module| module.name == name) {
        return Err(Errno::EEXIST);
    }

    let status = module.call_init()?;
    if status < 0 {
        return Ok(status);
    }

    let mut modules = MODULES.lock();
    if modules.iter().any(|module| module.name == name) {
        drop(modules);
        module.call_exit();
        return Err(Errno::EEXIST);
    }
    modules.push(LoadedModule { name, module });
    Ok(status)
}
