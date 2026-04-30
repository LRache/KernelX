// SPDX-License-Identifier: MIT
// Portions of this file are adapted from KXemu's PLIC device code.
// Original project: KXemu (MIT License), Copyright (c) 2024 HD-CSKX.
// Source repository: git@github.com:HD-CSKX/KXemu.git

#include "device/plic.hpp"

#include "device/bus.hpp"

#include <cstddef>
#include <cstdint>

namespace kvm_host {
namespace {
struct AddrSpace {
    std::uintptr_t base;
    std::uintptr_t size;

    bool contains(std::uintptr_t offset) const {
        return offset >= base && offset < base + size;
    }
};

constexpr AddrSpace PLIC_PRIORITY = {0x000000, 0x001000};
constexpr AddrSpace PLIC_PENDING = {0x001000, 0x000080};
constexpr AddrSpace PLIC_ENABLE = {0x002000, 0x1f0000};
constexpr AddrSpace PLIC_CONTEXT = {0x200000, 0x100000};

static bool is_aligned_u32_access(std::uintptr_t offset, std::size_t width) {
    return width == sizeof(std::uint32_t) && (offset & (sizeof(std::uint32_t) - 1)) == 0;
}
} // namespace

bool PlicDevice::read(std::uintptr_t offset, std::size_t width, std::uint64_t *value) {
    if (value == nullptr || !is_aligned_u32_access(offset, width)) {
        return false;
    }

    if (PLIC_PRIORITY.contains(offset)) {
        const std::size_t source = offset / sizeof(std::uint32_t);
        *value = source < kSourceCount ? interrupt_sources_[source].priority : 0;
        return true;
    }

    if (PLIC_PENDING.contains(offset)) {
        const std::size_t word = (offset - PLIC_PENDING.base) / sizeof(std::uint32_t);
        std::uint32_t pending = 0;
        for (std::size_t bit = 0; bit < 32; bit++) {
            const std::size_t source = word * 32 + bit;
            if (source < kSourceCount && interrupt_sources_[source].pending) {
                pending |= static_cast<std::uint32_t>(1u << bit);
            }
        }
        *value = pending;
        return true;
    }

    if (PLIC_ENABLE.contains(offset)) {
        const std::size_t context = (offset - PLIC_ENABLE.base) / 0x80;
        const std::size_t word = ((offset - PLIC_ENABLE.base) % 0x80) / sizeof(std::uint32_t);
        if (context >= kContextCount) {
            *value = 0;
            return true;
        }

        if (word != 0) {
            *value = 0;
            return true;
        }

        std::uint32_t enabled = 0;
        for (std::size_t source = 0; source < kSourceCount; source++) {
            if (interrupt_sources_[source].enable[context]) {
                enabled |= static_cast<std::uint32_t>(1u << source);
            }
        }
        *value = enabled;
        return true;
    }

    if (PLIC_CONTEXT.contains(offset)) {
        const std::size_t context = (offset - PLIC_CONTEXT.base) / 0x1000;
        const std::uintptr_t context_offset = (offset - PLIC_CONTEXT.base) % 0x1000;
        if (context >= kContextCount) {
            *value = 0;
            return true;
        }

        if (context_offset == 0) {
            *value = target_contexts_[context].threshold;
            return true;
        }
        if (context_offset == sizeof(std::uint32_t)) {
            *value = target_contexts_[context].claim;
            return true;
        }

        *value = 0;
        return true;
    }

    return false;
}

bool PlicDevice::write(std::uintptr_t offset, std::size_t width, std::uint64_t value) {
    if (!is_aligned_u32_access(offset, width)) {
        return false;
    }

    const std::uint32_t data = static_cast<std::uint32_t>(value);
    if (PLIC_PRIORITY.contains(offset)) {
        const std::size_t source = offset / sizeof(std::uint32_t);
        if (source < kSourceCount) {
            interrupt_sources_[source].priority = data;
        }
        return true;
    }

    if (PLIC_ENABLE.contains(offset)) {
        const std::size_t context = (offset - PLIC_ENABLE.base) / 0x80;
        const std::size_t word = ((offset - PLIC_ENABLE.base) % 0x80) / sizeof(std::uint32_t);
        if (context >= kContextCount || word != 0) {
            return true;
        }

        for (std::size_t source = 0; source < kSourceCount; source++) {
            interrupt_sources_[source].enable[context] = (data & (static_cast<std::uint32_t>(1u) << source)) != 0;
        }
        interrupt_sources_[0].enable[context] = false;
        return true;
    }

    if (PLIC_CONTEXT.contains(offset)) {
        const std::size_t context = (offset - PLIC_CONTEXT.base) / 0x1000;
        const std::uintptr_t context_offset = (offset - PLIC_CONTEXT.base) % 0x1000;
        if (context >= kContextCount) {
            return false;
        }

        if (context_offset == 0) {
            target_contexts_[context].threshold = data;
            return true;
        }
        if (context_offset == sizeof(std::uint32_t)) {
            if (data == target_contexts_[context].claim && data < kSourceCount) {
                interrupt_sources_[data].claimed = false;
                target_contexts_[context].claim = 0;
            }
            return true;
        }

        return false;
    }

    return false;
}

void PlicDevice::update() {
    if (bus_ == nullptr) {
        return;
    }

    scan_interrupt_sources();
    refresh_context_claims();
}

void PlicDevice::connect_to_bus(Bus *bus) {
    bus_ = bus;
}

const char *PlicDevice::type_name() const {
    return "plic";
}

void PlicDevice::scan_interrupt_sources() {
    if (bus_ == nullptr) {
        return;
    }

    for (std::size_t i = 0; i < bus_->mmio_device_count(); i++) {
        const Bus::MmioRegion *region = bus_->mmio_region_at(i);
        if (region == nullptr || region->device == nullptr || region->device.get() == this || region->id == 0 ||
            region->id >= kSourceCount) {
            continue;
        }

        if (region->device->interrupt_pending()) {
            interrupt_sources_[region->id].pending = true;
        }
    }
}

void PlicDevice::refresh_context_claims() {
    if (bus_ == nullptr) {
        return;
    }

    for (std::size_t context = 0; context < kContextCount; context++) {
        if (target_contexts_[context].claim != 0) {
            continue;
        }

        std::uint32_t best_source = 0;
        std::uint32_t best_priority = target_contexts_[context].threshold;
        for (std::uint32_t source = 1; source < kSourceCount; source++) {
            const InterruptSource &interrupt_source = interrupt_sources_[source];
            if (!interrupt_source.pending || interrupt_source.claimed || !interrupt_source.enable[context] ||
                interrupt_source.priority <= best_priority) {
                continue;
            }

            best_priority = interrupt_source.priority;
            best_source = source;
        }

        if (best_source == 0) {
            continue;
        }

        interrupt_sources_[best_source].pending = false;
        interrupt_sources_[best_source].claimed = true;
        target_contexts_[context].claim = best_source;

        for (std::size_t i = 0; i < bus_->mmio_device_count(); i++) {
            const Bus::MmioRegion *region = bus_->mmio_region_at(i);
            if (region == nullptr || region->device == nullptr || region->id != best_source) {
                continue;
            }

            region->device->clear_interrupt();
            break;
        }
    }
}
} // namespace kvm_host
