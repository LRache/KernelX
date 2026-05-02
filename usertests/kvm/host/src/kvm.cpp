#include "kvm.hpp"

#include <cstdio>
#include <cstring>
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
        reset_fd(&this->fd_, std::exchange(other.fd_, -1));
        this->bus_ = std::move(other.bus_);
    }
    return *this;
}

Kvm::~Kvm() {
    reset_fd(&this->fd_);
}

std::uint8_t *GuestMapping::translate(std::uintptr_t guest_addr, std::uintptr_t length) const {
    if (this->host_base == nullptr || length == 0 || guest_addr < this->guest_base) {
        return nullptr;
    }

    const std::uintptr_t offset = guest_addr - this->guest_base;
    if (offset > this->guest_size || length > this->guest_size - offset) {
        return nullptr;
    }

    return this->host_base + offset;
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
    return this->bus_ != nullptr && this->bus_->map_area(*this, addr, length, host_addr);
}

bool Kvm::add_memory(std::uintptr_t guest_size, GuestMapping *mapping_out) {
    if (mapping_out == nullptr) {
        return false;
    }

    std::uint8_t *host_addr = nullptr;
    if (!this->map_area(GUEST_MEMORY_BASE, guest_size, &host_addr)) {
        return false;
    }
    if (host_addr == nullptr) {
        std::fprintf(stderr, "guest memory is not mapped on bus\n");
        return false;
    }

    *mapping_out = {GUEST_MEMORY_BASE, guest_size, host_addr};
    return true;
}

bool Kvm::copy_to_guest(const GuestMapping &mapping, std::uintptr_t guest_addr, const std::uint8_t *data,
                        std::uintptr_t size, const char *label) const {
    if (data == nullptr || size == 0 || label == nullptr) {
        return false;
    }

    std::uint8_t *host_addr = mapping.translate(guest_addr, size);
    if (host_addr == nullptr) {
        std::fprintf(stderr, "guest %s does not fit in mapped memory: addr=0x%lx size=0x%lx\n", label,
                     static_cast<unsigned long>(guest_addr), static_cast<unsigned long>(size));
        return false;
    }

    std::memcpy(host_addr, data, static_cast<std::size_t>(size));
    std::printf("loaded %s: 0x%lx bytes to guest 0x%lx via host %p\n", label, static_cast<unsigned long>(size),
                static_cast<unsigned long>(guest_addr), host_addr);
    return true;
}

bool Kvm::add_mmio_device(std::uintptr_t guest_addr, std::uintptr_t length, std::shared_ptr<MmioDevice> device,
                          unsigned int id) {
    return this->bus_ != nullptr && this->bus_->add_mmio_device(guest_addr, length, std::move(device), id);
}

bool Kvm::map_area_raw(std::uintptr_t addr, std::uintptr_t length, std::uint8_t **host_addr) const {
    KvmMapArea area = {addr, length, 0};
    const long ret = ioctl(this->fd_, KVM_MAP_AREA, &area);
    if (ret < 0) {
        perror("ioctl(KVM_MAP_AREA)");
        return false;
    }

    *host_addr = reinterpret_cast<std::uint8_t *>(area.mapped_addr);
    return true;
}

bool Kvm::create_cpu(KvmCpu *out) {
    const long fd = ioctl(this->fd_, KVM_CREATE_VCPU, 0);
    if (fd < 0) {
        perror("ioctl(KVM_CREATE_VCPU)");
        return false;
    }

    *out = KvmCpu(static_cast<int>(fd), this->bus_);
    return true;
}

std::shared_ptr<Bus> Kvm::bus() const {
    return this->bus_;
}

int Kvm::raw_fd() const {
    return this->fd_;
}
} // namespace kvm_host
