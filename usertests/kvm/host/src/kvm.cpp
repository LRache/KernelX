#include "kvm.hpp"

#include <cstdio>
#include <fcntl.h>
#include <memory>
#include <sys/ioctl.h>
#include <unistd.h>
#include <utility>

namespace kvm_host {
static void reset_fd(int *slot, int fd = -1) {
    if (*slot >= 0) {
        close(*slot);
    }
    *slot = fd;
}

Kvm::Kvm() : bus_(std::make_shared<Bus>()) {}

Kvm::Kvm(int fd) : fd_(fd), bus_(std::make_shared<Bus>()) {}

Kvm::Kvm(Kvm &&other) noexcept : fd_(std::exchange(other.fd_, -1)), bus_(std::move(other.bus_)) {}

Kvm &Kvm::operator=(Kvm &&other) noexcept {
    if (this != &other) {
        reset_fd(&fd_, std::exchange(other.fd_, -1));
        bus_ = std::move(other.bus_);
    }
    return *this;
}

Kvm::~Kvm() {
    reset_fd(&fd_);
}

bool Kvm::open(Kvm *out) {
    const int fd = ::open("/dev/kvm", O_RDONLY);
    if (fd < 0) {
        perror("open /dev/kvm");
        return false;
    }

    *out = Kvm(fd);
    return true;
}

bool Kvm::map_area(std::uintptr_t addr, std::uintptr_t length, std::uint8_t **host_addr) {
    return bus_ != nullptr && bus_->map_area(*this, addr, length, host_addr);
}

bool Kvm::add_mmio_device(std::uintptr_t guest_addr, std::uintptr_t length, std::shared_ptr<MmioDevice> device,
                          unsigned int id) {
    return bus_ != nullptr && bus_->add_mmio_device(guest_addr, length, std::move(device), id);
}

bool Kvm::map_area_raw(std::uintptr_t addr, std::uintptr_t length, std::uint8_t **host_addr) const {
    KvmMapArea area = {addr, length, 0};
    const long ret = ioctl(fd_, KVM_MAP_AREA, &area);
    if (ret < 0) {
        perror("ioctl(KVM_MAP_AREA)");
        return false;
    }

    *host_addr = reinterpret_cast<std::uint8_t *>(area.mapped_addr);
    return true;
}

bool Kvm::create_cpu(KvmCpu *out) {
    const long fd = ioctl(fd_, KVM_CREATE_VCPU, 0);
    if (fd < 0) {
        perror("ioctl(KVM_CREATE_VCPU)");
        return false;
    }

    *out = KvmCpu(static_cast<int>(fd), bus_);
    return true;
}

std::shared_ptr<Bus> Kvm::bus() const {
    return bus_;
}

int Kvm::raw_fd() const {
    return fd_;
}
} // namespace kvm_host
