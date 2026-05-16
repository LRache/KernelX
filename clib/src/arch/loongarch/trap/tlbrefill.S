/*
 * LoongArch TLB refill handler (TLBRENTRY vector).
 *
 * Taken on any TLB miss. Separate from EENTRY; uses its own shadow CSRs
 * (TLBRERA, TLBRPRMD, TLBRBADV, TLBRELO{0,1}, TLBREHI, TLBRSAVE).
 *
 * Hardware contract on entry:
 *   - CRMD: PLV=0, IE=0, DA=1, PG=0 (MMU OFF — fetches are raw PAs)
 *   - TLBRERA / TLBRBADV: faulting PC / VA
 *   - CSR.PGD: auto-selected from PGDL/PGDH by TLBRBADV sign
 *   - TLBRSAVE: the one scratch CSR we have
 *
 * Because the MMU is off, the handler must sit at a fixed PA (see
 * `.balign 0x1000` below; Arch::init programs TLBRENTRY with that PA).
 * No stack, no memory references outside `lddir` / `ldpte` / `tlbfill`
 * which walk the page table rooted at CSR.PGD.
 *
 * PWCL layout (3-level 9-9-9-12, 4 KiB pages):
 *   lddir $t0, $t0, 2   ← Dir2 (top)
 *   lddir $t0, $t0, 1   ← Dir1 (mid)
 *   ldpte $t0, 0        ← leaf even page → TLBRELO0
 *   ldpte $t0, 1        ← leaf odd  page → TLBRELO1
 *   tlbfill             ← commit pair
 *
 * LoongArch TLB entries are PAIRED (even/odd, VA[12]). We must do both
 * `ldpte 0` and `ldpte 1` — otherwise the sibling page faults on the
 * next access.
 *
 * Any missing intermediate / invalid PTE during the walk triggers a
 * different exception (page fault etc.) served via EENTRY.
 */

    .section .text.tlbrefill, "ax", @progbits
    .balign  0x1000

    .global asm_tlb_refill_entry
    .type   asm_tlb_refill_entry, @function
asm_tlb_refill_entry:
    csrwr   $t0, 0x8b                  /* save $t0 into TLBRSAVE */

    csrrd   $t0, 0x1b                  /* $t0 = CSR.PGD */

    /*
     * Walk the 3-level tree. At each level we must check the intermediate
     * slot is non-zero before descending — if we deref zero we end up
     * walking physical page 0 and loading nonsense PTEs that hardware
     * will happily install into the TLB.
     *
     * LoongArch convention for leaf handling: jump into a fault-path stub
     * that forges a PTE with V=0 and runs tlbfill anyway, so the faulting
     * access raises PIL/PIS/PIF via the normal EENTRY dispatch.
     */
    lddir   $t0, $t0, 2
    beqz    $t0, 1f
    lddir   $t0, $t0, 1
    beqz    $t0, 1f

    ldpte   $t0, 0
    ldpte   $t0, 1

2:  tlbfill

    csrrd   $t0, 0x8b                  /* restore $t0 */
    ertn

    /*
     * Fault path: the upper directories had a zero slot. Forge a PTE with
     * PLV=3, MAT=CC, V=0, written to both TLBRELO0/1 so tlbfill leaves
     * an inert entry behind. The subsequent access will raise a real
     * page-fault through EENTRY.
     *
     * Encoding: rotate TLBREHI right by 61 (pulling PS into low bits),
     * OR in 0x3 (PLV=3, MAT=?), rotate back. The exact sequence matches
     * NPUcore's handler. PS is 0xC (4 KiB) — set via TLBREHI bit offset.
     */
1:  csrrd   $t0, 0x8e                  /* TLBREHI */
    ori     $t0, $t0, 0xC
    csrwr   $t0, 0x8e

    rotri.d $t0, $t0, 61
    ori     $t0, $t0, 3
    rotri.d $t0, $t0, 3

    csrwr   $t0, 0x8c                  /* TLBRELO0 */
    csrrd   $t0, 0x8c
    csrwr   $t0, 0x8d                  /* TLBRELO1 */
    b       2b
    .size asm_tlb_refill_entry, . - asm_tlb_refill_entry
