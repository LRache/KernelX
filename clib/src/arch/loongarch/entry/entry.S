/*
 * LoongArch64 boot entry.
 *
 * 1. Set DMW0 (VSEG 0x8, MAT=SUC) covering MMIO.
 * 2. Set DMW1 (VSEG 0x9, MAT=CC)  covering cached RAM.
 * 3. Jump into the DMW1 mirror of our own code (safe while CRMD.DA=1
 *    because the CPU masks top bits for fetch/load/store).
 * 4. CRMD.PG=1 / DA=0: MMU on. DMW1 keeps every kernel VA mapped to
 *    the same PA, so fetching continues smoothly.
 * 5. Clear BSS, point $sp at __stack_end, call main().
 *
 * QEMU's `loongarch_direct_kernel_boot` path hands us:
 *   $a0 = 1 (magic, not a hartid)
 *   $a1 = cmdline PA (LoongArch BPI; not an FDT pointer)
 *   $a2 = EFI sys table offset
 * The FDT blob is ROM-loaded at the fixed PA 0x100000 (see QEMU's
 * include/hw/loongarch/virt.h). We ignore $a0 (force hartid=0) and
 * stash $a1 into `la_boot_cmdline_pa` for fdt.rs.
 */

    .section .text.entry
    .global _entry
    .type _entry, @function
_entry:
    move    $s0, $zero                /* $s0 = hartid = 0 (forced) */
    move    $s1, $a1                  /* $s1 = cmdline PA */

    /* DMW0 @ 0x8000_...: PLV0, MAT=SUC. */
    ori     $t0, $zero, 0x1
    lu52i.d $t0, $t0, -2048
    csrwr   $t0, 0x180                /* CSR_DMW0 */

    /* DMW1 @ 0x9000_...: PLV0, MAT=CC. */
    ori     $t0, $zero, 0x11
    lu52i.d $t0, $t0, -1792
    csrwr   $t0, 0x181                /* CSR_DMW1 */

    /* Jump into DMW1 high half so the next fetch resolves via DMW1
     * once PG is on. */
    pcaddi  $t0, 3
    lu52i.d $t0, $t0, -1792
    jr      $t0
1:
    /* CRMD: PLV=0, IE=0, DA=0, PG=1, DATF=CC, DATM=CC. */
    ori     $t0, $zero, 0xb0
    csrwr   $t0, 0x0                  /* CSR_CRMD */

    /* Clear BSS. */
    la.pcrel $t0, __bss_start
    la.pcrel $t1, __bss_end
2:  beq     $t0, $t1, 3f
    st.d    $zero, $t0, 0
    addi.d  $t0, $t0, 8
    b       2b
3:

    /* Stash cmdline PA AFTER BSS clear (BSS zero-pass would otherwise
     * stomp it). Lives in .data so it's already preserved. */
    la.pcrel $t0, la_boot_cmdline_pa
    st.d    $s1, $t0, 0

    /* Kernel stack grows down from __stack_end. */
    la.pcrel $sp, __stack_end

    /*
     * main(hartid, heap_start, memory_top). heap_start and memory_top
     * are VAs in the DMW1 high half — kernel arithmetic stays in the
     * 64-bit linked space.
     *
     * memory_top = 0x9000_..._0000 | 0x1000_0000 (256 MiB). QEMU `-M virt`
     * gives 256 MiB at PA [0, 0x1000_0000); past that is MMIO and cached
     * loads there would raise ADE through DMW1. The frame allocator must
     * respect this cap.
     */
    move    $a0, $s0
    la.pcrel $a1, __kernel_end
    lu12i.w $a2, 0x10000                /* 0x1000_0000 */
    lu52i.d $a2, $a2, -1792             /* | 0x9000_..._0000 */

    la.global $t0, main
    jirl    $zero, $t0, 0

    /* main() never returns. */
4:  idle    0
    b       4b
    .size _entry, . - _entry

/* Secondary-core entry stub for SMP; weak until we have real bring-up. */
    .section .text.entry
    .global __la_others_entry
    .weak __la_others_entry
__la_others_entry:
1:  idle 0
    b 1b

/* Written by _entry after BSS clear; read by fdt.rs as the global
 * `la_boot_cmdline_pa`. In `.data` so BSS zero-pass leaves it alone. */
    .section .data
    .globl la_boot_cmdline_pa
    .balign 8
la_boot_cmdline_pa:
    .dword 0
