#include "kvm.hpp"
#include "device/plic.hpp"
#include "linux_dtb.hpp"
#include "device/uart.hpp"

#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <iostream>
#include <memory>
#include <string>
#include <unistd.h>
#include <vector>

namespace {
static constexpr std::uintptr_t GUEST_MEMORY_BASE = 0x80000000;
static constexpr std::uintptr_t LEGACY_GUEST_MEMORY_SIZE = 0x2000;
static constexpr std::uintptr_t LINUX_GUEST_MEMORY_SIZE = 0x08000000;
static constexpr std::uintptr_t LINUX_KERNEL_LOAD_ADDR = 0x80200000;
static constexpr std::uintptr_t LINUX_DTB_LOAD_ADDR = 0x82200000;
static constexpr std::uintptr_t LINUX_INITRAMFS_LOAD_ADDR = 0x84200000;

static constexpr std::uintptr_t UART0_BASE = 0x10000000;
static constexpr std::uintptr_t PLIC_BASE = 0x0c000000;
static constexpr std::uint32_t UART0_IRQ = 10;
static constexpr std::uintptr_t KVM_EXIT_SBI_CALL = 16;

static constexpr const char *DEFAULT_GUEST_IMAGE = "/guest/hello_sbi.bin";
static constexpr const char *DEFAULT_LINUX_KERNEL_IMAGE = "/guest/linux5.15/Image";
static constexpr const char *DEFAULT_LINUX_INITRAMFS_IMAGE = "/guest/linux5.15/initramfs.cpio.gz";
static constexpr const char *DEFAULT_LINUX_BOOTARGS = "earlycon=sbi console=ttyS0 loglevel=8";

struct GuestMapping {
    std::uintptr_t guest_base = 0;
    std::uintptr_t guest_size = 0;
    std::uint8_t *host_base = nullptr;

    std::uint8_t *translate(std::uintptr_t guest_addr, std::uintptr_t length) const {
        if (host_base == nullptr || length == 0 || guest_addr < guest_base) {
            return nullptr;
        }

        const std::uintptr_t offset = guest_addr - guest_base;
        if (offset > guest_size || length > guest_size - offset) {
            return nullptr;
        }

        return host_base + offset;
    }
};

struct LegacyBootOptions {
    const char *image_path = DEFAULT_GUEST_IMAGE;
};

struct LinuxBootOptions {
    const char *kernel_path = DEFAULT_LINUX_KERNEL_IMAGE;
    const char *initramfs_path = nullptr;
    const char *dtb_path = nullptr;
    std::string bootargs = DEFAULT_LINUX_BOOTARGS;
    std::uintptr_t memory_size = LINUX_GUEST_MEMORY_SIZE;
};

static bool file_exists(const char *path) {
    return path != nullptr && access(path, R_OK) == 0;
}

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

static bool parse_uintptr_arg(const char *text, std::uintptr_t *value) {
    if (text == nullptr || value == nullptr || *text == '\0') {
        return false;
    }

    char *end = nullptr;
    errno = 0;
    const unsigned long long parsed = std::strtoull(text, &end, 0);
    if (errno != 0 || end == text || *end != '\0') {
        return false;
    }

    *value = static_cast<std::uintptr_t>(parsed);
    return true;
}

static void print_usage(const char *argv0) {
    std::fprintf(stderr,
                 "Usage:\n"
                 "  %s [guest.bin]\n"
                 "  %s --linux [--kernel PATH] [--initramfs PATH|--no-initramfs]\n"
                 "             [--dtb PATH] [--bootargs STRING] [--memory-size BYTES]\n",
                 argv0, argv0);
}

static bool read_file_to_buffer(const char *path, std::vector<std::uint8_t> *buffer) {
    if (path == nullptr || buffer == nullptr) {
        return false;
    }

    const int fd = open(path, O_RDONLY);
    if (fd < 0) {
        std::fprintf(stderr, "open %s: %s\n", path, std::strerror(errno));
        return false;
    }

    buffer->clear();
    std::uint8_t chunk[4096];
    while (true) {
        const ssize_t ret = read(fd, chunk, sizeof(chunk));
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

        buffer->insert(buffer->end(), chunk, chunk + ret);
    }

    if (close(fd) < 0) {
        std::fprintf(stderr, "close %s: %s\n", path, std::strerror(errno));
        return false;
    }
    if (buffer->empty()) {
        std::fprintf(stderr, "guest image %s is empty\n", path);
        return false;
    }

    return true;
}

static bool copy_blob_to_guest(const std::uint8_t *data, std::uintptr_t size, const GuestMapping &mapping,
                               std::uintptr_t guest_addr, const char *label) {
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

static bool load_file_to_guest(const char *path, const GuestMapping &mapping, std::uintptr_t guest_addr,
                               const char *label, std::uintptr_t *size_out = nullptr) {
    std::vector<std::uint8_t> buffer;
    if (!read_file_to_buffer(path, &buffer)) {
        return false;
    }
    if (!copy_blob_to_guest(buffer.data(), static_cast<std::uintptr_t>(buffer.size()), mapping, guest_addr, label)) {
        return false;
    }

    if (size_out != nullptr) {
        *size_out = static_cast<std::uintptr_t>(buffer.size());
    }
    return true;
}

static bool create_guest_mapping(kvm_host::Kvm *kvm, std::uintptr_t guest_size, GuestMapping *mapping_out) {
    if (kvm == nullptr || mapping_out == nullptr) {
        return false;
    }

    std::uint8_t *host_addr = nullptr;
    if (!kvm->map_area(GUEST_MEMORY_BASE, guest_size, &host_addr)) {
        return false;
    }
    if (host_addr == nullptr) {
        std::fprintf(stderr, "guest memory is not mapped on bus\n");
        return false;
    }

    *mapping_out = {GUEST_MEMORY_BASE, guest_size, host_addr};
    return true;
}

static bool prepare_legacy_guest(const GuestMapping &mapping, const LegacyBootOptions &options, kvm_host::KvmRegs *regs) {
    std::uintptr_t image_size = 0;
    if (!load_file_to_guest(options.image_path, mapping, GUEST_MEMORY_BASE, options.image_path, &image_size)) {
        return false;
    }

    regs->pc = GUEST_MEMORY_BASE;
    regs->a0 = 0x1234;
    return true;
}

static bool prepare_linux_guest(const GuestMapping &mapping, const LinuxBootOptions &options, kvm_host::KvmRegs *regs) {
    std::uintptr_t kernel_size = 0;
    if (!load_file_to_guest(options.kernel_path, mapping, LINUX_KERNEL_LOAD_ADDR, options.kernel_path, &kernel_size)) {
        return false;
    }

    std::uintptr_t initramfs_end = 0;
    const char *initramfs_path = options.initramfs_path;
    if (initramfs_path != nullptr) {
        std::uintptr_t initramfs_size = 0;
        if (!load_file_to_guest(initramfs_path, mapping, LINUX_INITRAMFS_LOAD_ADDR, initramfs_path, &initramfs_size)) {
            return false;
        }
        initramfs_end = LINUX_INITRAMFS_LOAD_ADDR + initramfs_size;
    }

    std::vector<std::uint8_t> dtb_blob;
    if (options.dtb_path != nullptr) {
        if (!read_file_to_buffer(options.dtb_path, &dtb_blob)) {
            return false;
        }
    } else {
        kvm_host::LinuxGuestDtbConfig dtb = {};
        dtb.memory_base = GUEST_MEMORY_BASE;
        dtb.memory_size = options.memory_size;
        dtb.uart_base = UART0_BASE;
        dtb.uart_size = kvm_host::Uart16650Device::kLength;
        dtb.uart_irq = UART0_IRQ;
        dtb.plic_base = PLIC_BASE;
        dtb.plic_size = kvm_host::PlicDevice::kLength;
        dtb.plic_ndev = kvm_host::PlicDevice::kNumInterrupts;
        dtb.has_plic = true;
        dtb.bootargs = options.bootargs;
        dtb.stdout_path = "/soc/serial@10000000";
        dtb.has_initrd = initramfs_end != 0;
        dtb.initrd_start = LINUX_INITRAMFS_LOAD_ADDR;
        dtb.initrd_end = initramfs_end;
        dtb_blob = kvm_host::build_linux_guest_dtb(dtb);
    }

    if (!copy_blob_to_guest(dtb_blob.data(), static_cast<std::uintptr_t>(dtb_blob.size()), mapping, LINUX_DTB_LOAD_ADDR,
                            options.dtb_path != nullptr ? options.dtb_path : "built-in linux dtb")) {
        return false;
    }

    regs->pc = LINUX_KERNEL_LOAD_ADDR;
    regs->a0 = 0;
    regs->a1 = LINUX_DTB_LOAD_ADDR;
    return true;
}

static int boot_guest(const GuestMapping &mapping, const kvm_host::KvmRegs &boot_regs, kvm_host::Kvm *kvm) {
    kvm_host::KvmCpu cpu;
    if (!kvm->create_cpu(&cpu)) {
        return 1;
    }

    std::printf("kvm host test ready: /dev/kvm fd=%d, vcpu fd=%d\n", kvm->raw_fd(), cpu.raw_fd());
    std::printf("mapped guest memory: guest=[0x%lx,0x%lx) host=%p\n", static_cast<unsigned long>(mapping.guest_base),
                static_cast<unsigned long>(mapping.guest_base + mapping.guest_size), mapping.host_base);

    if (!cpu.set_regs(boot_regs)) {
        return 1;
    }

    kvm_host::KvmRegs verify = {};
    if (!cpu.get_regs(verify)) {
        return 1;
    }

    std::printf("vcpu pc=0x%lx a0=0x%lx a1=0x%lx after set_regs\n", static_cast<unsigned long>(verify.pc),
                static_cast<unsigned long>(verify.a0), static_cast<unsigned long>(verify.a1));

    std::uintptr_t exit_reason = 0;
    if (!cpu.run(&exit_reason)) {
        return 1;
    }
    print_exit_reason(exit_reason);
    return 0;
}
} // namespace

int main(int argc, char **argv) {
    bool linux_mode = false;
    LegacyBootOptions legacy = {};
    LinuxBootOptions linux = {};
    bool initramfs_explicit = false;
    bool kernel_explicit = false;
    bool legacy_image_explicit = false;

    for (int i = 1; i < argc; i++) {
        const std::string arg = argv[i];
        if (arg == "--help" || arg == "-h") {
            print_usage(argv[0]);
            return 0;
        }
        if (arg == "--linux") {
            linux_mode = true;
            continue;
        }
        if (arg == "--kernel") {
            if (i + 1 >= argc) {
                print_usage(argv[0]);
                return 1;
            }
            linux.kernel_path = argv[++i];
            linux_mode = true;
            kernel_explicit = true;
            continue;
        }
        if (arg == "--dtb") {
            if (i + 1 >= argc) {
                print_usage(argv[0]);
                return 1;
            }
            linux.dtb_path = argv[++i];
            linux_mode = true;
            continue;
        }
        if (arg == "--initramfs") {
            if (i + 1 >= argc) {
                print_usage(argv[0]);
                return 1;
            }
            linux.initramfs_path = argv[++i];
            linux_mode = true;
            initramfs_explicit = true;
            continue;
        }
        if (arg == "--no-initramfs") {
            linux.initramfs_path = nullptr;
            linux_mode = true;
            initramfs_explicit = true;
            continue;
        }
        if (arg == "--bootargs") {
            if (i + 1 >= argc) {
                print_usage(argv[0]);
                return 1;
            }
            linux.bootargs = argv[++i];
            linux_mode = true;
            continue;
        }
        if (arg == "--memory-size") {
            if (i + 1 >= argc || !parse_uintptr_arg(argv[++i], &linux.memory_size)) {
                std::fprintf(stderr, "invalid --memory-size value\n");
                return 1;
            }
            linux_mode = true;
            continue;
        }
        if (!linux_mode && !legacy_image_explicit) {
            legacy.image_path = argv[i];
            legacy_image_explicit = true;
            continue;
        }
        if (linux_mode && !kernel_explicit) {
            linux.kernel_path = argv[i];
            kernel_explicit = true;
            continue;
        }

        std::fprintf(stderr, "unexpected argument: %s\n", argv[i]);
        print_usage(argv[0]);
        return 1;
    }

    if (linux_mode && !initramfs_explicit && file_exists(DEFAULT_LINUX_INITRAMFS_IMAGE)) {
        linux.initramfs_path = DEFAULT_LINUX_INITRAMFS_IMAGE;
    }

    kvm_host::Kvm kvm;
    if (!kvm_host::Kvm::open(&kvm)) {
        return 1;
    }

    auto uart = std::make_shared<kvm_host::Uart16650Device>();
    uart->set_output_stream(std::cout);
    if (!kvm.add_mmio_device(UART0_BASE, kvm_host::Uart16650Device::kLength, std::move(uart), UART0_IRQ)) {
        return 1;
    }

    auto plic = std::make_shared<kvm_host::PlicDevice>();
    if (!kvm.add_mmio_device(PLIC_BASE, kvm_host::PlicDevice::kLength, std::move(plic))) {
        return 1;
    }

    const std::uintptr_t guest_size = linux_mode ? linux.memory_size : LEGACY_GUEST_MEMORY_SIZE;
    GuestMapping mapping = {};
    if (!create_guest_mapping(&kvm, guest_size, &mapping)) {
        return 1;
    }

    kvm_host::KvmRegs regs = {};
    if (linux_mode) {
        if (!prepare_linux_guest(mapping, linux, &regs)) {
            return 1;
        }
    } else {
        if (!prepare_legacy_guest(mapping, legacy, &regs)) {
            return 1;
        }
    }

    return boot_guest(mapping, regs, &kvm);
}
