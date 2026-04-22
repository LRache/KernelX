/*
 * LoongArch64 boot entry.
 *
 * Responsibilities in the current phase:
 *   1. Set DMW0 (VSEG 0x8, MAT=SUC) to cover MMIO (UART, PIC, RTC, ...).
 *   2. Set DMW1 (VSEG 0x9, MAT=CC)  to cover cached RAM (kernel image + heap).
 *   3. Jump from the low-half identity region into the DMW1 mirror of our
 *      own code. This is safe while CRMD.DA is still 1 because the CPU
 *      masks the top bits off for fetch/load/store.
 *   4. Flip CRMD.PG=1 / DA=0. From here on MMU is on, but DMW1 keeps
 *      every VA of ours mapped to the same PA, so we can keep executing.
 *   5. Clear BSS.
 *   6. Point $sp at __stack_end (in the DMW1 high half).
 *   7. Call `main(hartid, heap_start, memory_top)`.
 *
 * QEMU's aux_boot_code has already handed us:
 *   - $a0 = hartid (always 0 for BSP on the virt machine right now)
 *   - $a1 = FDT pointer (unused for now, Phase 6 will consume it)
 *   - $a2 = 0 (unused)
 *
 * We overwrite $a0/$a1/$a2 before calling into Rust so the signature
 * matches `kernel::main::main(hartid, heap_start, memory_top)`.
 */

    .section .text.entry
    .global _entry
    .type _entry, @function
_entry:
    /* Save hartid in a callee-saved temporary while we rebuild GPRs. */
    move    $s0, $a0                  /* $s0 = hartid */

    /* DMW0 @ 0x8000_0000_0000_0000: PLV0=1 (bit0), MAT=SUC (bits 5:4 = 0). */
    ori     $t0, $zero, 0x1
    lu52i.d $t0, $t0, -2048           /* bits 63:52 = 0x800 */
    csrwr   $t0, 0x180                /* CSR_DMW0 */

    /* DMW1 @ 0x9000_0000_0000_0000: PLV0=1 (bit0), MAT=CC (bits 5:4 = 1). */
    ori     $t0, $zero, 0x11
    lu52i.d $t0, $t0, -1792           /* bits 63:52 = 0x900 */
    csrwr   $t0, 0x181                /* CSR_DMW1 */

    /* Jump into DMW1 high half. DA=1 still; the CPU masks the top bits
     * off for the fetch but PC itself keeps them, so after we flip PG on
     * the next fetch the MMU resolves via DMW1 cleanly. */
    pcaddi  $t0, 3
    lu52i.d $t0, $t0, -1792           /* |= 0x9000 << 48 */
    jr      $t0
1:
    /* CRMD: PLV=0, IE=0, DA=0, PG=1 (bit4), DATF=CC (bits6:5=01),
     *       DATM=CC (bits8:7=01).  value = 0x10 | 0x20 | 0x80 = 0xb0 */
    ori     $t0, $zero, 0xb0
    csrwr   $t0, 0x0                  /* CSR_CRMD */

    /* Clear BSS. __bss_start/__bss_end are linker-provided VAs in DMW1. */
    la.pcrel $t0, __bss_start
    la.pcrel $t1, __bss_end
2:  beq     $t0, $t1, 3f
    st.d    $zero, $t0, 0
    addi.d  $t0, $t0, 8
    b       2b
3:

    /* Kernel stack grows down from __stack_end. */
    la.pcrel $sp, __stack_end

    /*
     * Build the argument triple for Rust main:
     *   a0 = hartid          (preserved in $s0)
     *   a1 = heap_start      (__kernel_end, aligned to 4 KiB)
     *   a2 = memory_top      (DMW1 base + 256 MiB; Phase 6 will read this from FDT)
     *
     * heap_start and memory_top are VAs in the DMW1 high half so that the
     * kernel's address-arithmetic stays in the same 64-bit space it was
     * linked at. `kaddr_to_paddr` / `paddr_to_kaddr` will do the translation
     * on demand later.
     */
    move    $a0, $s0
    la.pcrel $a1, __kernel_end
    /* memory_top = 0x9000_0000_0000_0000 | 0x1000_0000  (256 MiB) */
    lu12i.w $a2, 0x10000              /* 0x10000000 */
    lu52i.d $a2, $a2, -1792           /* |= 0x9000 << 48 */

    /* Hand off to Rust. `main` is declared `extern "C" fn main(...)` in
     * src/kernel/main.rs. pcaddu18i + jirl reaches anywhere within +-128 MiB
     * which is plenty. la.global does the same but goes through GOT. */
    la.global $t0, main
    jirl    $zero, $t0, 0

    /* Unreachable: main() never returns. Park if it does. */
4:  idle    0
    b       4b
    .size _entry, . - _entry

/*
 * Placeholder for secondary-core entry, referenced by Phase 8's SMP bring-up.
 * Keep as a weak symbol until then so linking still works.
 */
    .section .text.entry
    .global __la_others_entry
    .weak __la_others_entry
__la_others_entry:
1:  idle 0
    b 1b
