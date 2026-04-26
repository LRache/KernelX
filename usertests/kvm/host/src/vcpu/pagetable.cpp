#include "vcpu.hpp"

#include "bus.hpp"

#include <cstdio>
#include <cstring>
#include <limits>

namespace kvm_host {
static constexpr std::uintptr_t PAGE_SHIFT = 12;
static constexpr std::uintptr_t PAGE_SIZE = 1 << PAGE_SHIFT;
static constexpr std::uintptr_t PTE_SIZE = 8;
static constexpr std::uintptr_t VPN_BITS = 9;
static constexpr std::uintptr_t VPN_MASK = (1 << VPN_BITS) - 1;
static constexpr std::uintptr_t SATP_MODE_BARE = 0;
static constexpr std::uintptr_t SATP_MODE_SV39 = 8;
static constexpr std::uintptr_t SATP_MODE_SHIFT = 60;
static constexpr std::uintptr_t SATP_PPN_MASK = (static_cast<std::uintptr_t>(1) << 44) - 1;

static constexpr std::uint64_t PTE_V = 1 << 0;
static constexpr std::uint64_t PTE_R = 1 << 1;
static constexpr std::uint64_t PTE_W = 1 << 2;
static constexpr std::uint64_t PTE_X = 1 << 3;
static constexpr std::uint64_t PTE_PPN_MASK = (static_cast<std::uint64_t>(1) << 44) - 1;

static bool checked_range_end(std::uintptr_t start, std::uintptr_t length, std::uintptr_t *end) {
    if (length == 0) {
        return false;
    }
    if (start > std::numeric_limits<std::uintptr_t>::max() - length) {
        return false;
    }

    *end = start + length;
    return true;
}

static bool is_sv39_canonical(std::uintptr_t vaddr) {
    static constexpr std::uintptr_t SIGN_BIT = static_cast<std::uintptr_t>(1) << 38;
    static constexpr std::uintptr_t LOWER_MASK = (static_cast<std::uintptr_t>(1) << 39) - 1;
    const std::uintptr_t upper = vaddr & ~LOWER_MASK;

    if ((vaddr & SIGN_BIT) == 0) {
        return upper == 0;
    }
    return upper == ~LOWER_MASK;
}

static std::uintptr_t vpn_index(std::uintptr_t vaddr, int level) {
    return (vaddr >> (PAGE_SHIFT + static_cast<std::uintptr_t>(level) * VPN_BITS)) & VPN_MASK;
}

static bool read_pte(const Bus &bus, std::uintptr_t paddr, std::uint64_t *pte) {
    std::uint8_t *host_addr = bus.translate(paddr, PTE_SIZE);
    if (host_addr == nullptr) {
        return false;
    }

    std::uint64_t value = 0;
    std::memcpy(&value, host_addr, sizeof(value));
    *pte = value;
    return true;
}

static bool is_valid_pte(std::uint64_t pte) {
    const bool valid = (pte & PTE_V) != 0;
    const bool readable = (pte & PTE_R) != 0;
    const bool writable = (pte & PTE_W) != 0;

    return valid && (readable || !writable);
}

static bool is_leaf_pte(std::uint64_t pte) {
    return (pte & (PTE_R | PTE_X)) != 0;
}

static std::uintptr_t pte_ppn(std::uint64_t pte) {
    return static_cast<std::uintptr_t>((pte >> 10) & PTE_PPN_MASK);
}

static bool range_fits_in_leaf(std::uintptr_t vaddr, std::uintptr_t length, int level) {
    const std::uintptr_t page_size = PAGE_SIZE << (static_cast<std::uintptr_t>(level) * VPN_BITS);
    const std::uintptr_t page_offset = vaddr & (page_size - 1);
    return length <= page_size - page_offset;
}

std::uint8_t *KvmCpu::translate_guest_vaddr(std::uintptr_t guest_vaddr, std::uintptr_t length) const {
    if (bus_ == nullptr) {
        return nullptr;
    }

    std::uintptr_t end = 0;
    if (!checked_range_end(guest_vaddr, length, &end)) {
        return nullptr;
    }

    KvmSRegs sregs = {};
    if (!get_sregs(sregs)) {
        return nullptr;
    }

    const std::uintptr_t satp_mode = sregs.satp >> SATP_MODE_SHIFT;
    if (satp_mode == SATP_MODE_BARE) {
        return bus_->translate(guest_vaddr, length);
    }
    if (satp_mode != SATP_MODE_SV39) {
        std::fprintf(stderr, "unsupported guest satp mode: 0x%lx\n", static_cast<unsigned long>(satp_mode));
        return nullptr;
    }
    if (!is_sv39_canonical(guest_vaddr) || !is_sv39_canonical(end - 1)) {
        return nullptr;
    }

    std::uintptr_t table_paddr = (sregs.satp & SATP_PPN_MASK) << PAGE_SHIFT;
    for (int level = 2; level >= 0; level--) {
        const std::uintptr_t pte_addr = table_paddr + vpn_index(guest_vaddr, level) * PTE_SIZE;
        std::uint64_t pte = 0;
        if (!read_pte(*bus_, pte_addr, &pte) || !is_valid_pte(pte)) {
            return nullptr;
        }

        const std::uintptr_t ppn = pte_ppn(pte);
        if (is_leaf_pte(pte)) {
            const std::uintptr_t lower_ppn_mask =
                (static_cast<std::uintptr_t>(1) << (static_cast<std::uintptr_t>(level) * VPN_BITS)) - 1;
            if ((ppn & lower_ppn_mask) != 0 || !range_fits_in_leaf(guest_vaddr, length, level)) {
                return nullptr;
            }

            const std::uintptr_t page_shift = PAGE_SHIFT + static_cast<std::uintptr_t>(level) * VPN_BITS;
            const std::uintptr_t page_offset = guest_vaddr & ((static_cast<std::uintptr_t>(1) << page_shift) - 1);
            const std::uintptr_t guest_paddr = (ppn << PAGE_SHIFT) | page_offset;
            return bus_->translate(guest_paddr, length);
        }

        table_paddr = ppn << PAGE_SHIFT;
    }

    return nullptr;
}
} // namespace kvm_host
