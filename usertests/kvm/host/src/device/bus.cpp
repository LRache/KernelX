// SPDX-License-Identifier: MIT
// Portions of this file are adapted from KXemu's MMIO bus code.
// Original project: KXemu (MIT License), Copyright (c) 2024 HD-CSKX.
// Source repository: git@github.com:HD-CSKX/KXemu.git

#include "device/bus.hpp"

#include "kvm.hpp"
#include "linux_dtb.hpp"

#include <cstdio>
#include <limits>
#include <utility>

namespace kvm_host {
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

static bool ranges_overlap(std::uintptr_t left_start, std::uintptr_t left_length, std::uintptr_t right_start,
                           std::uintptr_t right_length) {
    std::uintptr_t left_end = 0;
    std::uintptr_t right_end = 0;
    if (!checked_range_end(left_start, left_length, &left_end) ||
        !checked_range_end(right_start, right_length, &right_end)) {
        return true;
    }

    return left_start < right_end && right_start < left_end;
}

bool Bus::map_area(const Kvm &kvm, std::uintptr_t guest_addr, std::uintptr_t length, std::uint8_t **mapped_host_addr) {
    std::uintptr_t end = 0;
    if (!checked_range_end(guest_addr, length, &end)) {
        std::fprintf(stderr, "bus map area has invalid range: addr=0x%lx length=0x%lx\n",
                     static_cast<unsigned long>(guest_addr), static_cast<unsigned long>(length));
        return false;
    }

    if (this->area_count_ == this->MAX_AREAS) {
        std::fprintf(stderr, "bus map area table is full\n");
        return false;
    }

    for (std::size_t i = 0; i < this->area_count_; i++) {
        if (ranges_overlap(guest_addr, length, this->areas_[i].guest_addr, this->areas_[i].length)) {
            std::fprintf(stderr, "bus map area overlaps existing range: new=[0x%lx,0x%lx) old=[0x%lx,0x%lx)\n",
                         static_cast<unsigned long>(guest_addr), static_cast<unsigned long>(end),
                         static_cast<unsigned long>(this->areas_[i].guest_addr),
                         static_cast<unsigned long>(this->areas_[i].guest_addr + this->areas_[i].length));
            return false;
        }
    }
    for (std::size_t i = 0; i < this->mmio_device_count_; i++) {
        if (ranges_overlap(guest_addr, length, this->mmio_regions_[i].guest_addr, this->mmio_regions_[i].length)) {
            std::fprintf(stderr, "bus map area overlaps mmio device: addr=0x%lx length=0x%lx\n",
                         static_cast<unsigned long>(guest_addr), static_cast<unsigned long>(length));
            return false;
        }
    }

    std::uint8_t *host_addr = nullptr;
    if (!kvm.map_area_raw(guest_addr, length, &host_addr)) {
        return false;
    }
    if (host_addr == nullptr) {
        std::fprintf(stderr, "ioctl(KVM_MAP_AREA) returned null host mapping\n");
        return false;
    }

    this->areas_[this->area_count_] = {guest_addr, length, host_addr};
    this->area_count_++;
    if (mapped_host_addr != nullptr) {
        *mapped_host_addr = host_addr;
    }
    return true;
}

bool Bus::add_mmio_device(std::uintptr_t guest_addr, std::uintptr_t length, std::shared_ptr<MmioDevice> device,
                          unsigned int id) {
    if (device == nullptr) {
        std::fprintf(stderr, "bus mmio device is null\n");
        return false;
    }

    std::uintptr_t end = 0;
    if (!checked_range_end(guest_addr, length, &end)) {
        std::fprintf(stderr, "bus mmio device has invalid range: addr=0x%lx length=0x%lx\n",
                     static_cast<unsigned long>(guest_addr), static_cast<unsigned long>(length));
        return false;
    }

    if (this->mmio_device_count_ == this->MAX_MMIO_DEVICES) {
        std::fprintf(stderr, "bus mmio device table is full\n");
        return false;
    }

    for (std::size_t i = 0; i < this->area_count_; i++) {
        if (ranges_overlap(guest_addr, length, this->areas_[i].guest_addr, this->areas_[i].length)) {
            std::fprintf(stderr, "bus mmio device overlaps memory area: new=[0x%lx,0x%lx) old=[0x%lx,0x%lx)\n",
                         static_cast<unsigned long>(guest_addr), static_cast<unsigned long>(end),
                         static_cast<unsigned long>(this->areas_[i].guest_addr),
                         static_cast<unsigned long>(this->areas_[i].guest_addr + this->areas_[i].length));
            return false;
        }
    }
    for (std::size_t i = 0; i < this->mmio_device_count_; i++) {
        if (ranges_overlap(guest_addr, length, this->mmio_regions_[i].guest_addr, this->mmio_regions_[i].length)) {
            std::fprintf(stderr, "bus mmio device overlaps existing device: new=[0x%lx,0x%lx)\n",
                         static_cast<unsigned long>(guest_addr), static_cast<unsigned long>(end));
            return false;
        }
    }

    device->connect_to_bus(this);
    this->mmio_regions_[this->mmio_device_count_] = {guest_addr, length, id, std::move(device)};
    this->mmio_device_count_++;
    return true;
}

bool Bus::read_mmio(std::uintptr_t guest_addr, std::size_t width, std::uint64_t *value) const {
    const MmioRegion *region = this->find_mmio_region(guest_addr, static_cast<std::uintptr_t>(width));
    if (region == nullptr || region->device == nullptr) {
        return false;
    }

    return region->device->read(guest_addr - region->guest_addr, width, value);
}

bool Bus::write_mmio(std::uintptr_t guest_addr, std::size_t width, std::uint64_t value) const {
    const MmioRegion *region = this->find_mmio_region(guest_addr, static_cast<std::uintptr_t>(width));
    if (region == nullptr || region->device == nullptr) {
        return false;
    }

    return region->device->write(guest_addr - region->guest_addr, width, value);
}

std::vector<std::uint8_t> Bus::build_dtb(const DtbConfig &config) const {
    DtbBuilder builder;
    builder.config_dtb(config);

    for (std::size_t i = 0; i < this->mmio_device_count_; i++) {
        const MmioRegion &region = this->mmio_regions_[i];
        if (region.device == nullptr) {
            continue;
        }

        region.device->config_dtb(builder, config, region.guest_addr, region.length, region.id);
    }

    return builder.finish_dtb();
}

std::uint8_t *Bus::translate(std::uintptr_t guest_addr, std::uintptr_t length) const {
    const Area *area = this->find_area(guest_addr, length);
    if (area == nullptr) {
        return nullptr;
    }

    return area->host_addr + (guest_addr - area->guest_addr);
}

const Bus::Area *Bus::find_area(std::uintptr_t guest_addr, std::uintptr_t length) const {
    std::uintptr_t end = 0;
    if (!checked_range_end(guest_addr, length, &end)) {
        return nullptr;
    }

    for (std::size_t i = 0; i < this->area_count_; i++) {
        const Area &area = this->areas_[i];
        const std::uintptr_t area_end = area.guest_addr + area.length;
        if (area.guest_addr <= guest_addr && end <= area_end) {
            return &area;
        }
    }

    return nullptr;
}

const Bus::MmioRegion *Bus::mmio_region_at(std::size_t index) const {
    if (index >= this->mmio_device_count_) {
        return nullptr;
    }

    return &this->mmio_regions_[index];
}

std::size_t Bus::mmio_device_count() const {
    return this->mmio_device_count_;
}

const Bus::MmioRegion *Bus::find_mmio_region(std::uintptr_t guest_addr, std::uintptr_t length) const {
    std::uintptr_t end = 0;
    if (!checked_range_end(guest_addr, length, &end)) {
        return nullptr;
    }

    for (std::size_t i = 0; i < this->mmio_device_count_; i++) {
        const MmioRegion &region = this->mmio_regions_[i];
        const std::uintptr_t region_end = region.guest_addr + region.length;
        if (region.guest_addr <= guest_addr && end <= region_end) {
            return &region;
        }
    }

    return nullptr;
}

void Bus::update() {
    for (std::size_t i = 0; i < this->mmio_device_count_; i++) {
        if (this->mmio_regions_[i].device != nullptr) {
            this->mmio_regions_[i].device->update();
        }
    }
}

std::size_t Bus::area_count() const {
    return this->area_count_;
}
} // namespace kvm_host
