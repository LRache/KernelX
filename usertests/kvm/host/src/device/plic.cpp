// SPDX-License-Identifier: MIT
// Portions of this file are adapted from KXemu's PLIC device code.
// Original project: KXemu (MIT License), Copyright (c) 2024 HD-CSKX.
// Source repository: git@github.com:HD-CSKX/KXemu.git

#include "device/plic.hpp"

#include "device/bus.hpp"
#include "linux_dtb.hpp"

#include <cstddef>
#include <cstdint>

namespace kvm_host {
namespace {
struct AddrSpace {
    std::uintptr_t base;
    std::uintptr_t size;

    bool contains(std::uintptr_t offset) const {
        return offset >= this->base && offset < this->base + this->size;
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
        *value = source < this->kSourceCount ? this->interrupt_sources_[source].priority : 0;
        return true;
    }

    if (PLIC_PENDING.contains(offset)) {
        const std::size_t word = (offset - PLIC_PENDING.base) / sizeof(std::uint32_t);
        std::uint32_t pending = 0;
        for (std::size_t bit = 0; bit < 32; bit++) {
            const std::size_t source = word * 32 + bit;
            if (source < this->kSourceCount && this->interrupt_sources_[source].pending) {
                pending |= static_cast<std::uint32_t>(1u << bit);
            }
        }
        *value = pending;
        return true;
    }

    if (PLIC_ENABLE.contains(offset)) {
        const std::size_t context = (offset - PLIC_ENABLE.base) / 0x80;
        const std::size_t word = ((offset - PLIC_ENABLE.base) % 0x80) / sizeof(std::uint32_t);
        if (context >= this->kContextCount) {
            *value = 0;
            return true;
        }

        if (word != 0) {
            *value = 0;
            return true;
        }

        std::uint32_t enabled = 0;
        for (std::size_t source = 0; source < this->kSourceCount; source++) {
            if (this->interrupt_sources_[source].enable[context]) {
                enabled |= static_cast<std::uint32_t>(1u << source);
            }
        }
        *value = enabled;
        return true;
    }

    if (PLIC_CONTEXT.contains(offset)) {
        const std::size_t context = (offset - PLIC_CONTEXT.base) / 0x1000;
        const std::uintptr_t context_offset = (offset - PLIC_CONTEXT.base) % 0x1000;
        if (context >= this->kContextCount) {
            *value = 0;
            return true;
        }

        if (context_offset == 0) {
            *value = this->target_contexts_[context].threshold;
            return true;
        }
        if (context_offset == sizeof(std::uint32_t)) {
            *value = this->target_contexts_[context].claim;
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
        if (source < this->kSourceCount) {
            this->interrupt_sources_[source].priority = data;
        }
        return true;
    }

    if (PLIC_ENABLE.contains(offset)) {
        const std::size_t context = (offset - PLIC_ENABLE.base) / 0x80;
        const std::size_t word = ((offset - PLIC_ENABLE.base) % 0x80) / sizeof(std::uint32_t);
        if (context >= this->kContextCount || word != 0) {
            return true;
        }

        for (std::size_t source = 0; source < this->kSourceCount; source++) {
            this->interrupt_sources_[source].enable[context] = (data & (static_cast<std::uint32_t>(1u) << source)) != 0;
        }
        this->interrupt_sources_[0].enable[context] = false;
        return true;
    }

    if (PLIC_CONTEXT.contains(offset)) {
        const std::size_t context = (offset - PLIC_CONTEXT.base) / 0x1000;
        const std::uintptr_t context_offset = (offset - PLIC_CONTEXT.base) % 0x1000;
        if (context >= this->kContextCount) {
            return false;
        }

        if (context_offset == 0) {
            this->target_contexts_[context].threshold = data;
            return true;
        }
        if (context_offset == sizeof(std::uint32_t)) {
            if (data == this->target_contexts_[context].claim && data < this->kSourceCount) {
                this->interrupt_sources_[data].claimed = false;
                this->target_contexts_[context].claim = 0;
            }
            return true;
        }

        return false;
    }

    return false;
}

void PlicDevice::update() {
    if (this->bus_ == nullptr) {
        return;
    }

    this->scan_interrupt_sources();
    this->refresh_context_claims();
}

void PlicDevice::connect_to_bus(Bus *bus) {
    this->bus_ = bus;
}

void PlicDevice::config_dtb(DtbBuilder &builder, const DtbConfig &config, std::uintptr_t guest_addr,
                                     std::uintptr_t length, unsigned int id) const {
    (void)id;
    builder.begin_node(dtb_node_name("plic", guest_addr));
    builder.prop_u32("phandle", config.plic_phandle);
    builder.prop_u32("riscv,ndev", this->kNumInterrupts);
    builder.prop_cells("reg", dtb_reg_cells(guest_addr, length));
    builder.prop_cells("interrupts-extended", {config.cpu_intc_phandle, 11, config.cpu_intc_phandle, 9});
    builder.prop_bool("interrupt-controller");
    builder.prop_string_list("compatible", {"sifive,plic-1.0.0", "riscv,plic0"});
    builder.prop_u32("#address-cells", 0);
    builder.prop_u32("#interrupt-cells", 1);
    builder.end_node();
}

const char *PlicDevice::type_name() const {
    return "plic";
}

void PlicDevice::scan_interrupt_sources() {
    if (this->bus_ == nullptr) {
        return;
    }

    for (std::size_t i = 0; i < this->bus_->mmio_device_count(); i++) {
        const Bus::MmioRegion *region = this->bus_->mmio_region_at(i);
        if (region == nullptr || region->device == nullptr || region->device.get() == this || region->id == 0 ||
            region->id >= this->kSourceCount) {
            continue;
        }

        if (region->device->interrupt_pending()) {
            this->interrupt_sources_[region->id].pending = true;
        }
    }
}

void PlicDevice::refresh_context_claims() {
    if (this->bus_ == nullptr) {
        return;
    }

    for (std::size_t context = 0; context < this->kContextCount; context++) {
        if (this->target_contexts_[context].claim != 0) {
            continue;
        }

        std::uint32_t best_source = 0;
        std::uint32_t best_priority = this->target_contexts_[context].threshold;
        for (std::uint32_t source = 1; source < this->kSourceCount; source++) {
            const InterruptSource &interrupt_source = this->interrupt_sources_[source];
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

        this->interrupt_sources_[best_source].pending = false;
        this->interrupt_sources_[best_source].claimed = true;
        this->target_contexts_[context].claim = best_source;

        for (std::size_t i = 0; i < this->bus_->mmio_device_count(); i++) {
            const Bus::MmioRegion *region = this->bus_->mmio_region_at(i);
            if (region == nullptr || region->device == nullptr || region->id != best_source) {
                continue;
            }

            region->device->clear_interrupt();
            break;
        }
    }
}
} // namespace kvm_host
