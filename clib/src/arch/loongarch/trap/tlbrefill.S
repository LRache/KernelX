/*
 * LoongArch TLB refill handler.
 *
 * LoongArch has a dedicated exception vector for TLB refill — TLBRENTRY
 * — that is taken when a memory access misses both the MTLB (fully
 * associative) and the STLB (set associative). It is *not* part of the
 * generic EENTRY vector and has its own CSR save-area / shadow state
 * (TLBRERA, TLBRPRMD, TLBRBADV, TLBRELO{0,1}, TLBREHI, TLBRSAVE).
 *
 * ## Hardware contract on entry
 *
 *   - CRMD: PLV=0, IE=0, DA=1, PG=0 (MMU OFF) — that's the big one.
 *   - TLBRERA holds the PC of the faulting access.
 *   - TLBRBADV holds the faulting VA.
 *   - CSR.PGD was auto-selected from PGDL / PGDH based on TLBRBADV's
 *     sign bit, so we can `csrrd` it without extra logic.
 *   - TLBRSAVE is our one scratch slot (we can't use the kernel stack —
 *     no DMW, no MMU).
 *
 * Consequences for the handler:
 *   - Every instruction's fetch is a bare PA access. So the handler
 *     itself must sit at a fixed PA the loader can name, and every
 *     memory reference inside must be PA-resolvable. We pin it to its
 *     own 4 KiB page via `.balign 0x1000`; Rust writes that PA into
 *     TLBRENTRY during Arch::init.
 *   - We can use `lddir` / `ldpte` because those operate through
 *     physical pointers anyway (they walk the page-table structure
 *     rooted at `csrrd PGD`, whose contents are PAs per LoongArch
 *     architectural requirements).
 *
 * ## Page-table layout we serve
 *
 *   - 3-level 9-9-9-12 on 4 KiB pages with 64-bit PTEs.
 *   - PWCL encodes the walker levels as: PT=Dir0 (leaf, bits 12..20),
 *     Dir1 (bits 21..29), Dir2 (bits 30..38). PWCH is 0 (no Dir3/4).
 *     So:
 *        lddir $t0, $t0, 2   ← consume Dir2 (PGD → PMD entry)
 *        lddir $t0, $t0, 1   ← consume Dir1 (PMD → PT-pair entry)
 *        ldpte $t0, 0        ← leaf even page → TLBRELO0
 *        ldpte $t0, 1        ← leaf odd  page → TLBRELO1
 *     `tlbfill` then writes the pair into the STLB and we `ertn`.
 *
 * LoongArch TLB entries are PAIRED: one entry maps two consecutive 4 KiB
 * pages (even/odd, indexed by VA[12]). `ldpte 0` / `ldpte 1` must both
 * be issued; issuing only one leaves the sibling page invalid and we
 * relock on the very next access.
 *
 * ## Why this is minimal
 *
 * We use:
 *   - `csrwr $t0, TLBRSAVE`  to save/restore $t0 atomically (1 slot).
 *   - No stack, no heap, no memory scribbling outside what `lddir` /
 *     `ldpte` / `tlbfill` do on their own.
 *
 * If any of the walker instructions discover an absent intermediate
 * table or invalid PTE, they fault — execution drops out of this
 * handler into a different vector (TLB modify / page-fault etc.), which
 * EENTRY handles via usertrap_handler. That is the path page-fault
 * servicing takes today.
 */

    .section .text.tlbrefill, "ax", @progbits
    .balign  0x1000

    .global asm_tlb_refill_entry
    .type   asm_tlb_refill_entry, @function
asm_tlb_refill_entry:
    /* Save $t0 into TLBRSAVE so we have one scratch register to work
     * with without touching user / kernel state. The paired read later
     * restores it atomically. */
    csrwr   $t0, 0x8b                  /* CSR.TLBRSAVE ← $t0 (old→TLBRSAVE) */

    /* $t0 = PGD (CSR 0x1b auto-selects PGDL or PGDH from TLBRBADV). */
    csrrd   $t0, 0x1b                  /* CSR.PGD */

    /* Walk Dir2 (PGD) and Dir1 (PMD). In our PWCL layout:
     *   Dir2 = level 2 (top directory),
     *   Dir1 = level 1 (middle directory),
     *   PT   = leaf (Dir0 implicitly).
     * After the two lddirs, $t0 points at the pair of leaf PTEs. */
    lddir   $t0, $t0, 2
    lddir   $t0, $t0, 1

    /* Read the even/odd PTE pair into TLBRELO0/1. ldpte encodes the
     * PFN + permission bits in the format tlbfill expects. */
    ldpte   $t0, 0
    ldpte   $t0, 1

    /* Commit the new TLB entry. tlbfill picks a victim (round-robin
     * within the set the STLB index hashes to). */
    tlbfill

    /* Restore $t0 from TLBRSAVE. Using csrrd (not csrwr) leaves
     * TLBRSAVE unchanged, which is fine — its contents are undefined
     * between refill invocations. */
    csrrd   $t0, 0x8b

    ertn
    .size asm_tlb_refill_entry, . - asm_tlb_refill_entry
