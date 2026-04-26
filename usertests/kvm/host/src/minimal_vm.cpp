#include "kvm.hpp"
#include "uart.hpp"

#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <iostream>
#include <memory>
#include <unistd.h>

static constexpr std::uintptr_t GUEST_ENTRY = 0x80000000;
static constexpr std::uintptr_t GUEST_MEMORY_SIZE = 0x2000;
static constexpr std::uintptr_t UART0_BASE = 0x10000000;
static constexpr std::uintptr_t TEST_A0 = 0x1234;
static constexpr std::uintptr_t KVM_EXIT_SBI_CALL = 16;
static constexpr const char *DEFAULT_GUEST_IMAGE = "/guest/hello_sbi.bin";

static const char *memory_access_name(std::uintptr_t access_type) {
    switch (access_type) {
        case 0:
            return "read";
        case 1:
            return "write";
        case 2:
            return "execute";
        default:
            return "unknown";
    }
}

static void print_exit_reason(std::uintptr_t reason) {
    std::printf("kvm exit reason raw=0x%lx\n", static_cast<unsigned long>(reason));

    if (reason == KVM_EXIT_SBI_CALL) {
        std::printf("kvm exit: return_to_user sbi_call\n");
        return;
    }

    if ((reason & ~static_cast<std::uintptr_t>(0x3)) != 0) {
        const std::uintptr_t addr = reason >> 2;
        const std::uintptr_t access_type = reason & 0x3;
        std::printf("kvm exit: memory_fault addr=0x%lx access=%s\n", static_cast<unsigned long>(addr),
                    memory_access_name(access_type));
        return;
    }

    std::printf("kvm exit: other code=%lu\n", static_cast<unsigned long>(reason));
}

static bool load_binary_image(const char *path, std::uint8_t *memory, std::uintptr_t memory_size,
                              std::uintptr_t *image_size) {
    const int fd = open(path, O_RDONLY);
    if (fd < 0) {
        std::fprintf(stderr, "open %s: %s\n", path, std::strerror(errno));
        return false;
    }

    *image_size = 0;
    while (true) {
        if (*image_size == memory_size) {
            std::uint8_t extra = 0;
            const ssize_t ret = read(fd, &extra, sizeof(extra));
            if (ret == 0) {
                break;
            }
            if (ret < 0) {
                if (errno == EINTR) {
                    continue;
                }
                std::fprintf(stderr, "read %s: %s\n", path, std::strerror(errno));
                close(fd);
                return false;
            }

            std::fprintf(stderr, "guest image %s is larger than 0x%lx bytes\n", path, memory_size);
            close(fd);
            return false;
        }

        const ssize_t ret = read(fd, memory + *image_size, static_cast<size_t>(memory_size - *image_size));
        if (ret == 0) {
            break;
        }
        if (ret < 0) {
            if (errno == EINTR) {
                continue;
            }
            std::fprintf(stderr, "read %s: %s\n", path, std::strerror(errno));
            close(fd);
            return false;
        }

        *image_size += static_cast<std::uintptr_t>(ret);
    }

    if (close(fd) < 0) {
        std::fprintf(stderr, "close %s: %s\n", path, std::strerror(errno));
        return false;
    }

    if (*image_size == 0) {
        std::fprintf(stderr, "guest image %s is empty\n", path);
        return false;
    }

    return true;
}

int main(int argc, char **argv) {
    const char *guest_image = argc > 1 ? argv[1] : DEFAULT_GUEST_IMAGE;

    kvm_host::Kvm kvm;
    if (!kvm_host::Kvm::open(&kvm)) {
        return 1;
    }
    auto uart = std::make_shared<kvm_host::Uart16650Device>();
    uart->set_output_stream(std::cout);
    if (!kvm.add_mmio_device(UART0_BASE, kvm_host::Uart16650Device::kLength, std::move(uart))) {
        return 1;
    }

    std::uint8_t *guest_memory = nullptr;
    if (!kvm.map_area(GUEST_ENTRY, GUEST_MEMORY_SIZE, &guest_memory)) {
        return 1;
    }
    if (guest_memory == nullptr) {
        std::fprintf(stderr, "guest memory is not mapped on bus\n");
        return 1;
    }

    std::uintptr_t image_size = 0;
    if (!load_binary_image(guest_image, guest_memory, GUEST_MEMORY_SIZE, &image_size)) {
        return 1;
    }

    kvm_host::KvmCpu cpu;
    if (!kvm.create_cpu(&cpu)) {
        return 1;
    }

    std::printf("kvm host test ready: /dev/kvm fd=%d, vcpu fd=%d\n", kvm.raw_fd(), cpu.raw_fd());
    std::printf("loaded %s: 0x%lx bytes to guest 0x%lx via host %p\n", guest_image,
                static_cast<unsigned long>(image_size), static_cast<unsigned long>(GUEST_ENTRY), guest_memory);

    kvm_host::KvmRegs regs = {};
    if (!cpu.get_regs(regs)) {
        return 1;
    }

    regs.pc = GUEST_ENTRY;
    regs.a0 = TEST_A0;
    if (!cpu.set_regs(regs)) {
        return 1;
    }

    kvm_host::KvmRegs verify = {};
    if (!cpu.get_regs(verify)) {
        return 1;
    }

    std::printf("vcpu pc=0x%lx a0=0x%lx after set_regs\n", static_cast<unsigned long>(verify.pc),
                static_cast<unsigned long>(verify.a0));

    std::uintptr_t exit_reason = 0;
    if (!cpu.run(&exit_reason)) {
        return 1;
    }
    print_exit_reason(exit_reason);

    return 0;
}
