#include "guest_boot.hpp"
#include "kvm.hpp"
#include "device/plic.hpp"
#include "device/uart.hpp"

#include <cstdio>
#include <iostream>
#include <memory>
#include <string>
#include <sys/ioctl.h>
#include <termios.h>
#include <utility>
#include <unistd.h>

namespace {
class StdinTermiosGuard {
public:
    StdinTermiosGuard() = default;
    StdinTermiosGuard(const StdinTermiosGuard &) = delete;
    StdinTermiosGuard &operator=(const StdinTermiosGuard &) = delete;

    ~StdinTermiosGuard() {
        if (this->enabled_) {
            (void)ioctl(STDIN_FILENO, TCSETS, &this->saved_);
        }
    }

    void enable_raw_input() {
        if (ioctl(STDIN_FILENO, TCGETS, &this->saved_) != 0) {
            return;
        }

        termios raw = this->saved_;
        raw.c_iflag &= static_cast<tcflag_t>(~(BRKINT | INPCK | ISTRIP | IXON | INLCR | IGNCR));
        raw.c_iflag |= ICRNL;
        raw.c_lflag &= static_cast<tcflag_t>(~(ICANON | ECHO | ISIG | IEXTEN));
        raw.c_cc[VMIN] = 1;
        raw.c_cc[VTIME] = 0;
        if (ioctl(STDIN_FILENO, TCSETS, &raw) == 0) {
            this->enabled_ = true;
        }
    }

private:
    bool enabled_ = false;
    termios saved_ = {};
};
} // namespace

int main(int argc, char **argv) {
    bool kernel_explicit = false;
    kvm_host::GuestBootOptions boot = {};

    for (int i = 1; i < argc; i++) {
        const std::string arg = argv[i];
        if (arg == "--help" || arg == "-h") {
            kvm_host::print_usage(argv[0]);
            return 0;
        }
        if (arg == "-kernel" || arg == "--kernel") {
            if (i + 1 >= argc) {
                std::fprintf(stderr, "%s requires a path argument\n", arg.c_str());
                kvm_host::print_usage(argv[0]);
                return 1;
            }
            boot.kernel_path = argv[++i];
            kernel_explicit = true;
            continue;
        }
        if (arg == "-dtb" || arg == "--dtb") {
            if (i + 1 >= argc) {
                std::fprintf(stderr, "%s requires a path argument\n", arg.c_str());
                kvm_host::print_usage(argv[0]);
                return 1;
            }
            boot.dtb_path = argv[++i];
            continue;
        }
        if (arg == "-initrd" || arg == "--initrd" || arg == "--initramfs") {
            if (i + 1 >= argc) {
                std::fprintf(stderr, "%s requires a path argument\n", arg.c_str());
                kvm_host::print_usage(argv[0]);
                return 1;
            }
            boot.initrd_path = argv[++i];
            continue;
        }
        if (arg == "--no-initrd" || arg == "--no-initramfs") {
            boot.initrd_path = nullptr;
            continue;
        }
        if (arg == "-append" || arg == "--append" || arg == "--bootargs") {
            if (i + 1 >= argc) {
                std::fprintf(stderr, "%s requires a string argument\n", arg.c_str());
                kvm_host::print_usage(argv[0]);
                return 1;
            }
            boot.bootargs = argv[++i];
            continue;
        }
        if (arg == "--memory-size") {
            if (i + 1 >= argc || !kvm_host::parse_uintptr_arg(argv[++i], &boot.memory_size)) {
                std::fprintf(stderr, "invalid --memory-size value\n");
                return 1;
            }
            continue;
        }
        if (!kernel_explicit) {
            boot.kernel_path = argv[i];
            kernel_explicit = true;
            continue;
        }

        std::fprintf(stderr, "unexpected argument: %s\n", argv[i]);
        kvm_host::print_usage(argv[0]);
        return 1;
    }

    kvm_host::Kvm kvm;
    if (!kvm_host::Kvm::open(&kvm)) {
        std::fprintf(stderr, "failed to open kvm device\n");
        return 1;
    }

    std::shared_ptr<kvm_host::Bus> bus = kvm.bus();
    if (bus == nullptr) {
        std::fprintf(stderr, "kvm bus is not available\n");
        return 1;
    }

    auto uart = std::make_shared<kvm_host::Uart16650Device>();
    uart->set_output_stream(std::cout);
    StdinTermiosGuard stdin_termios_guard;
    stdin_termios_guard.enable_raw_input();
    if (!bus->add_mmio_device(kvm_host::UART0_BASE, kvm_host::Uart16650Device::kLength, std::move(uart),
                              kvm_host::UART0_IRQ)) {
        std::fprintf(stderr, "failed to add uart mmio device\n");
        return 1;
    }

    auto plic = std::make_shared<kvm_host::PlicDevice>();
    if (!bus->add_mmio_device(kvm_host::PLIC_BASE, kvm_host::PlicDevice::kLength, std::move(plic))) {
        std::fprintf(stderr, "failed to add plic mmio device\n");
        return 1;
    }

    kvm_host::GuestMapping mapping = {};
    if (!kvm.add_memory(boot.memory_size, &mapping)) {
        std::fprintf(stderr, "failed to add guest memory: size=0x%lx\n", static_cast<unsigned long>(boot.memory_size));
        return 1;
    }

    kvm_host::GuestEntry entry = {};
    if (!kvm_host::prepare_guest(kvm, mapping, boot, *bus, &entry)) {
        std::fprintf(stderr, "failed to prepare guest image and dtb\n");
        return 1;
    }

    return kvm_host::boot_guest(mapping, entry, &kvm);
}
