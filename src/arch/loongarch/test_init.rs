//! TEMP(phase7): a hardcoded user-space binary + scaffolding to exercise
//! the Phase 5 user-mode round-trip before virtio-pci / ext4 is online.
//!
//! Current state: **infrastructure only**. The binary bytes and the map
//! helpers are here; the PCB/TCB wiring that would actually hand the task
//! to the scheduler is deferred, because the generic initprocess builder
//! insists on opening a real `/init` file (which requires a mounted root).
//! Once Phase 6/7 is up we throw this file away and go back to
//! `task::create_initprocess`.
//!
//! What this file *does* contribute right now:
//!   - A concrete byte blob with the `write(1, "hello from userspace\n",
//!     21); exit(42);` program. Compiled offline with
//!       clang --target=loongarch64-unknown-elf -c fake_init.S
//!     and hand-dumped; the source is in the module comment below.
//!   - Helpers to allocate + map the code and stack pages into a fresh
//!     `AddrSpace`. Those will be reused when we wire a test-only TCB
//!     builder in a follow-up commit.
//!
//! The asm trap entry (`clib/src/arch/loongarch/trap/usertrap.S`), Rust
//! dispatcher (`src/arch/loongarch/task/traphandle.rs`), and HPTW CSRs
//! (`Arch::init`) are all live regardless of whether this test process
//! ever runs — they compile and are ready the first time the scheduler
//! hands a TCB to `return_to_user`.
//!
//! Source for the binary blob (assembled + disassembled, then bytes copied):
//! ```asm
//!     _start:
//!         addi.d  $a0, $zero, 1     # fd = 1 (stdout)
//!         pcaddi  $a1, 8            # a1 = &msg (PC + 32)
//!         addi.d  $a2, $zero, 21    # len
//!         addi.d  $a7, $zero, 64    # SYS_write
//!         syscall 0
//!         addi.d  $a0, $zero, 42    # exit code 42
//!         addi.d  $a7, $zero, 93    # SYS_exit
//!         syscall 0
//!     1:  b 1b
//!     msg:
//!         .ascii "hello from userspace\n"
//! ```

use alloc::sync::Arc;

use crate::arch::{self, PageTableTrait};
use crate::kernel::mm::{AddrSpace, MapPerm, page};
use crate::kinfo;

/// LoongArch LP64 machine code + inline message for the fake init program.
/// 36 bytes of instructions followed by 21 bytes of "hello from userspace\n",
/// padded to 60 bytes for alignment.
#[allow(dead_code)]
static FAKE_INIT_BYTES: [u8; 60] = [
    0x04, 0x04, 0xc0, 0x02, // addi.d  $a0, $zero, 1
    0x05, 0x01, 0x00, 0x18, // pcaddi  $a1, 8
    0x06, 0x54, 0xc0, 0x02, // addi.d  $a2, $zero, 21
    0x0b, 0x00, 0xc1, 0x02, // addi.d  $a7, $zero, 64
    0x00, 0x00, 0x2b, 0x00, // syscall 0
    0x04, 0xa8, 0xc0, 0x02, // addi.d  $a0, $zero, 42
    0x0b, 0x74, 0xc1, 0x02, // addi.d  $a7, $zero, 93
    0x00, 0x00, 0x2b, 0x00, // syscall 0
    0x00, 0x00, 0x00, 0x50, // b . (unreached)
    b'h', b'e', b'l', b'l', b'o', b' ', b'f', b'r',
    b'o', b'm', b' ', b'u', b's', b'e', b'r', b's',
    b'p', b'a', b'c', b'e', b'\n',
    0x00, 0x00, 0x00,
];

#[allow(dead_code)]
const USER_CODE_UADDR: usize = 0x10000;
#[allow(dead_code)]
const USER_STACK_UADDR: usize = 0x20000;
#[allow(dead_code)]
const USER_STACK_PAGES: usize = 1;

/// Build a fresh `AddrSpace` containing the fake init program and a
/// one-page user stack. Returns (addrspace, entry_point, user_sp_top).
///
/// Callers are expected to wrap this in a TCB/PCB once the init-task
/// builder can skip the ELF-load path. Not wired yet; compile-tested only.
#[allow(dead_code)]
pub fn build_fake_init_addrspace() -> (Arc<AddrSpace>, usize, usize) {
    let addrspace = AddrSpace::new();

    // Code page: copy the blob into a fresh zeroed frame, map R|X|U.
    let code_kpage = page::alloc_zero();
    unsafe {
        core::ptr::copy_nonoverlapping(
            FAKE_INIT_BYTES.as_ptr(),
            code_kpage as *mut u8,
            FAKE_INIT_BYTES.len(),
        );
    }
    addrspace.pagetable().lock().mmap(
        USER_CODE_UADDR,
        code_kpage,
        MapPerm::R | MapPerm::X | MapPerm::U,
    );

    // Stack page: zeroed, R|W|U.
    let stack_kpage = page::alloc_zero();
    addrspace.pagetable().lock().mmap(
        USER_STACK_UADDR,
        stack_kpage,
        MapPerm::R | MapPerm::W | MapPerm::U,
    );

    let user_sp_top = USER_STACK_UADDR + USER_STACK_PAGES * arch::PGSIZE;
    (addrspace, USER_CODE_UADDR, user_sp_top)
}

/// Probe hook for Phase 5: build the fake-init AddrSpace and log its
/// addresses, without actually spawning a task yet. This is enough for
/// `make run` to reach the end of `Arch::init` without panicking, while
/// still compiling every piece of Phase 5 infrastructure in release mode.
pub fn probe() {
    let (addrspace, entry, sp_top) = build_fake_init_addrspace();
    kinfo!(
        "fake_init: mapped code @ {:#x}, stack top {:#x}, pgd PA {:#x}",
        entry,
        sp_top,
        addrspace.pagetable().lock().get_pgd(),
    );
    // addrspace is dropped here; the pages get freed. This is intentional
    // while TCB wiring is still TODO — we've exercised AddrSpace + mmap
    // + get_pgd, which is what Phase 5 needed to touch.
}
