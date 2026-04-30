// SPDX-License-Identifier: MIT
// Portions of this file are adapted from KXemu's PLIC device code.
// Original project: KXemu (MIT License), Copyright (c) 2024 HD-CSKX.
// Source repository: git@github.com:HD-CSKX/KXemu.git

#pragma once

#include "device/mmio.hpp"

#include <cstddef>
#include <cstdint>

namespace kvm_host {
class PlicDevice final : public MmioDevice {
public:
    static constexpr std::uintptr_t kLength = 0x04000000;
    static constexpr std::size_t kSourceCount = 32;
    static constexpr std::size_t kContextCount = 32;
    static constexpr std::uint32_t kNumInterrupts = static_cast<std::uint32_t>(kSourceCount - 1);

    bool read(std::uintptr_t offset, std::size_t width, std::uint64_t *value) override;
    bool write(std::uintptr_t offset, std::size_t width, std::uint64_t value) override;
    void update() override;
    void connect_to_bus(Bus *bus) override;
    const char *type_name() const override;

private:
    struct InterruptSource {
        std::uint32_t priority = 0;
        bool pending = false;
        bool claimed = false;
        bool enable[kContextCount] = {};
    };

    struct TargetContext {
        std::uint32_t threshold = 0;
        std::uint32_t claim = 0;
    };

    void scan_interrupt_sources();
    void refresh_context_claims();

    Bus *bus_ = nullptr;
    InterruptSource interrupt_sources_[kSourceCount] = {};
    TargetContext target_contexts_[kContextCount] = {};
};
} // namespace kvm_host
