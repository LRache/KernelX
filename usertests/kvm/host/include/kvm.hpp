#pragma once

#include "device/bus.hpp"
#include "vcpu.hpp"

#include <cstdint>
#include <memory>

namespace kvm_host {
class MmioDevice;

class Kvm {
public:
    Kvm();

    Kvm(const Kvm &) = delete;
    Kvm &operator=(const Kvm &) = delete;

    Kvm(Kvm &&other) noexcept;
    Kvm &operator=(Kvm &&other) noexcept;

    ~Kvm();

    static bool open(Kvm *out);

    bool map_area(std::uintptr_t addr, std::uintptr_t length, std::uint8_t **host_addr = nullptr);
    bool add_mmio_device(std::uintptr_t guest_addr, std::uintptr_t length, std::shared_ptr<MmioDevice> device,
                         unsigned int id = 0);
    bool create_cpu(KvmCpu *out);
    std::shared_ptr<Bus> bus() const;
    int raw_fd() const;

private:
    explicit Kvm(int fd);

    friend class Bus;

    bool map_area_raw(std::uintptr_t addr, std::uintptr_t length, std::uint8_t **host_addr) const;

    int fd_ = -1;
    std::shared_ptr<Bus> bus_;
};
} // namespace kvm_host
