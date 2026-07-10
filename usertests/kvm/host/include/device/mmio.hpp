// SPDX-License-Identifier: MIT
// Portions of this file are adapted from KXemu's MMIO device code.
// Original project: KXemu (MIT License), Copyright (c) 2024 HD-CSKX.
// Source repository: git@github.com:HD-CSKX/KXemu.git

#pragma once

#include <cstddef>
#include <cstdint>

namespace kvm_host {
class Bus;
class DtbBuilder;
struct DtbConfig;

class MmioDevice {
public:
    virtual ~MmioDevice() = default;

    virtual void reset() {}
    virtual bool read(std::uintptr_t offset, std::size_t width, std::uint64_t *value) = 0;
    virtual bool write(std::uintptr_t offset, std::size_t width, std::uint64_t value) = 0;
    virtual void update() {}
    virtual bool interrupt_pending() {
        return false;
    }
    virtual void clear_interrupt() {}
    virtual void connect_to_bus(Bus *bus) {
        (void)bus;
    }
    virtual void config_dtb(DtbBuilder &builder, const DtbConfig &config, std::uintptr_t guest_addr,
                            std::uintptr_t length, unsigned int id) const {
        (void)builder;
        (void)config;
        (void)guest_addr;
        (void)length;
        (void)id;
    }
    virtual const char *type_name() const {
        return "mmio";
    }
};
} // namespace kvm_host
