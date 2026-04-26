// SPDX-License-Identifier: MIT
// Portions of this file are adapted from KXemu's MMIO bus code.
// Original project: KXemu (MIT License), Copyright (c) 2024 HD-CSKX.
// Source repository: git@github.com:HD-CSKX/KXemu.git

#pragma once

#include "mmio.hpp"

#include <cstddef>
#include <cstdint>
#include <memory>

namespace kvm_host {
class Kvm;

class Bus {
public:
    struct Area {
        std::uintptr_t guest_addr = 0;
        std::uintptr_t length = 0;
        std::uint8_t *host_addr = nullptr;
    };

    struct MmioRegion {
        std::uintptr_t guest_addr = 0;
        std::uintptr_t length = 0;
        unsigned int id = 0;
        std::shared_ptr<MmioDevice> device;
    };

    Bus() = default;

    Bus(const Bus &) = delete;
    Bus &operator=(const Bus &) = delete;

    bool map_area(const Kvm &kvm, std::uintptr_t guest_addr, std::uintptr_t length,
                  std::uint8_t **host_addr = nullptr);
    bool add_mmio_device(std::uintptr_t guest_addr, std::uintptr_t length, std::shared_ptr<MmioDevice> device,
                         unsigned int id = 0);
    bool read_mmio(std::uintptr_t guest_addr, std::size_t width, std::uint64_t *value) const;
    bool write_mmio(std::uintptr_t guest_addr, std::size_t width, std::uint64_t value) const;
    std::uint8_t *translate(std::uintptr_t guest_addr, std::uintptr_t length) const;
    const Area *find_area(std::uintptr_t guest_addr, std::uintptr_t length) const;
    void update();
    std::size_t area_count() const;

private:
    static constexpr std::size_t MAX_AREAS = 16;
    static constexpr std::size_t MAX_MMIO_DEVICES = 16;

    const MmioRegion *find_mmio_region(std::uintptr_t guest_addr, std::uintptr_t length) const;

    Area areas_[MAX_AREAS] = {};
    std::size_t area_count_ = 0;
    MmioRegion mmio_regions_[MAX_MMIO_DEVICES] = {};
    std::size_t mmio_device_count_ = 0;
};
} // namespace kvm_host
