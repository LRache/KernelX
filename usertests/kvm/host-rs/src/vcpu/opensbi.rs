use std::io::{self, Write};

use num_enum::TryFromPrimitive;

use crate::abi::{KvmReg, KvmRegs};
use crate::vcpu::{KvmCpu, SbiCallResult};

pub fn handle_sbi_call(cpu: &KvmCpu, regs: KvmRegs) -> SbiCallResult {
    let eid = regs.get(KvmReg::A7);
    match SbiExtensionId::try_from(eid) {
        Ok(SbiExtensionId::LegacyConsolePutchar) => {
            print!("{}", regs.get(KvmReg::A0) as u8 as char);
            let _ = io::stdout().flush();
            finish_legacy_call(cpu, regs)
        }
        Ok(SbiExtensionId::LegacyConsoleGetchar) => {
            let mut next = regs;
            next.set(KvmReg::Pc, next.get(KvmReg::Pc) + 4);
            next.set(KvmReg::A0, usize::MAX);
            result_to_sbi(cpu.set_regs(&next))
        }
        Ok(SbiExtensionId::LegacyShutdown) => {
            println!("kvm exit: riscv sbi shutdown");
            SbiCallResult::Shutdown
        }
        Ok(SbiExtensionId::Base) => handle_base_extension(cpu, regs),
        Ok(SbiExtensionId::DebugConsole) => handle_debug_console_extension(cpu, regs),
        Ok(SbiExtensionId::RemoteFence) => handle_remote_fence_extension(cpu, regs),
        Ok(SbiExtensionId::SystemReset) => {
            let result = handle_system_reset_extension(cpu, regs);
            match result {
                SbiCallResult::Resume => SbiCallResult::Shutdown,
                other => other,
            }
        }
        Ok(SbiExtensionId::LegacySetTimer) => unsupported_legacy_call(eid, regs),
        Ok(SbiExtensionId::Time) => result_to_sbi(finish_sbi_call(cpu, regs, SbiError::NotSupported, 0)),
        Err(_) if eid >= SbiExtensionId::Base as usize => {
            result_to_sbi(finish_sbi_call(cpu, regs, SbiError::NotSupported, 0))
        }
        Err(_) => unsupported_legacy_call(eid, regs),
    }
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum SbiExtensionId {
    LegacySetTimer = 0x0,
    LegacyConsolePutchar = 0x1,
    LegacyConsoleGetchar = 0x2,
    LegacyShutdown = 0x8,
    Base = 0x10,
    Time = 0x5449_4d45,
    RemoteFence = 0x5246_4e43,
    SystemReset = 0x5352_5354,
    DebugConsole = 0x4442_434e,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum SbiBaseFunctionId {
    GetSpecVersion = 0,
    GetImplId = 1,
    GetImplVersion = 2,
    ProbeExtension = 3,
    GetMvendorId = 4,
    GetMarchId = 5,
    GetMimpId = 6,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum SbiDebugConsoleFunctionId {
    ConsoleWrite = 0,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum SbiRemoteFenceFunctionId {
    RemoteFenceI = 0,
    RemoteSfenceVma = 1,
    RemoteSfenceVmaAsid = 2,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum SbiSystemResetFunctionId {
    Reset = 0,
}

#[repr(isize)]
#[derive(Clone, Copy)]
enum SbiError {
    Success = 0,
    NotSupported = -2,
    InvalidAddress = -5,
}

fn finish_legacy_call(cpu: &KvmCpu, regs: KvmRegs) -> SbiCallResult {
    let mut next = regs;
    next.set(KvmReg::Pc, next.get(KvmReg::Pc) + 4);
    result_to_sbi(cpu.set_regs(&next))
}

fn unsupported_legacy_call(eid: usize, regs: KvmRegs) -> SbiCallResult {
    eprintln!(
        "unsupported riscv legacy sbi call: eid=0x{eid:x} fid=0x{:x}",
        regs.get(KvmReg::A6)
    );
    SbiCallResult::Failed
}

fn finish_sbi_call(cpu: &KvmCpu, regs: KvmRegs, error: SbiError, value: usize) -> Result<(), String> {
    let mut next = regs;
    next.set(KvmReg::Pc, next.get(KvmReg::Pc) + 4);
    next.set(KvmReg::A0, error as usize);
    next.set(KvmReg::A1, value);
    cpu.set_regs(&next)
}

fn result_to_sbi(result: Result<(), String>) -> SbiCallResult {
    match result {
        Ok(()) => SbiCallResult::Resume,
        Err(err) => {
            eprintln!("{err}");
            SbiCallResult::Failed
        }
    }
}

fn handle_base_extension(cpu: &KvmCpu, regs: KvmRegs) -> SbiCallResult {
    let value = match SbiBaseFunctionId::try_from(regs.get(KvmReg::A6)) {
        Ok(SbiBaseFunctionId::GetSpecVersion) => 3,
        Ok(SbiBaseFunctionId::GetImplId) => 1,
        Ok(SbiBaseFunctionId::GetImplVersion) => 0,
        Ok(SbiBaseFunctionId::ProbeExtension) => usize::from(is_extension_supported(regs.get(KvmReg::A0))),
        Ok(SbiBaseFunctionId::GetMvendorId) | Ok(SbiBaseFunctionId::GetMarchId) | Ok(SbiBaseFunctionId::GetMimpId) => 0,
        Err(_) => return result_to_sbi(finish_sbi_call(cpu, regs, SbiError::NotSupported, 0)),
    };
    result_to_sbi(finish_sbi_call(cpu, regs, SbiError::Success, value))
}

fn is_extension_supported(extension_id: usize) -> bool {
    SbiExtensionId::try_from(extension_id).is_ok()
}

fn handle_debug_console_extension(cpu: &KvmCpu, regs: KvmRegs) -> SbiCallResult {
    match SbiDebugConsoleFunctionId::try_from(regs.get(KvmReg::A6)) {
        Ok(SbiDebugConsoleFunctionId::ConsoleWrite) => {
            if write_guest_buffer_to_stdout(cpu, regs.get(KvmReg::A1), regs.get(KvmReg::A0)) {
                result_to_sbi(finish_sbi_call(cpu, regs, SbiError::Success, regs.get(KvmReg::A0)))
            } else {
                result_to_sbi(finish_sbi_call(cpu, regs, SbiError::InvalidAddress, 0))
            }
        }
        Err(_) => result_to_sbi(finish_sbi_call(cpu, regs, SbiError::NotSupported, 0)),
    }
}

fn write_guest_buffer_to_stdout(cpu: &KvmCpu, mut guest_addr: usize, mut length: usize) -> bool {
    const PAGE_SIZE: usize = 4096;
    let mut stdout = io::stdout();
    let mut buffer = [0u8; PAGE_SIZE];
    while length != 0 {
        let page_remaining = PAGE_SIZE - (guest_addr & (PAGE_SIZE - 1));
        let chunk = length.min(page_remaining);
        let data = &mut buffer[..chunk];
        if !cpu.translate_read(guest_addr, data) {
            return false;
        }
        if stdout.write_all(data).is_err() {
            return false;
        }
        guest_addr += chunk;
        length -= chunk;
    }
    stdout.flush().is_ok()
}

fn handle_remote_fence_extension(cpu: &KvmCpu, regs: KvmRegs) -> SbiCallResult {
    match SbiRemoteFenceFunctionId::try_from(regs.get(KvmReg::A6)) {
        Ok(SbiRemoteFenceFunctionId::RemoteFenceI)
        | Ok(SbiRemoteFenceFunctionId::RemoteSfenceVma)
        | Ok(SbiRemoteFenceFunctionId::RemoteSfenceVmaAsid) => {
            result_to_sbi(finish_sbi_call(cpu, regs, SbiError::Success, 0))
        }
        Err(_) => result_to_sbi(finish_sbi_call(cpu, regs, SbiError::NotSupported, 0)),
    }
}

fn handle_system_reset_extension(cpu: &KvmCpu, regs: KvmRegs) -> SbiCallResult {
    match SbiSystemResetFunctionId::try_from(regs.get(KvmReg::A6)) {
        Ok(SbiSystemResetFunctionId::Reset) => {
            println!(
                "kvm exit: riscv sbi system reset type={} reason={}",
                regs.get(KvmReg::A0),
                regs.get(KvmReg::A1)
            );
            result_to_sbi(finish_sbi_call(cpu, regs, SbiError::Success, 0))
        }
        Err(_) => result_to_sbi(finish_sbi_call(cpu, regs, SbiError::NotSupported, 0)),
    }
}
