/*
 * Phase 1 skeleton.
 *
 * ENTRY(_entry) in scripts/linker/loongarch64.ld needs *some* global symbol
 * so linking succeeds. This is just a placeholder that spins forever. The
 * real boot sequence (DMW setup, stack, C/Rust handoff) lands in Phase 2,
 * alongside TLB/exception vector programming in Phase 4.
 */

    .section .text.entry
    .global _entry
    .type _entry, @function
_entry:
1:  idle 0
    b 1b
    .size _entry, . - _entry

/*
 * Placeholder for secondary-core entry, referenced by Phase 8's SMP bring-up.
 * Keep as a weak symbol until then so linking the Phase 1 kernel still works.
 */
    .global __la_others_entry
    .weak __la_others_entry
__la_others_entry:
1:  idle 0
    b 1b
