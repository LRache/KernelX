#include "guest_boot.hpp"

#include "kvm.hpp"
#include "linux_dtb.hpp"

#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <optional>
#include <unistd.h>
#include <vector>

namespace kvm_host {
namespace {
bool read_file_to_buffer(const char *path, std::vector<std::uint8_t> *buffer) {
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

bool load_file_to_guest(const Kvm &kvm, const char *path, const GuestMapping &mapping, std::uintptr_t guest_addr,
                        const char *label, std::uintptr_t *size_out = nullptr) {
    std::vector<std::uint8_t> buffer;
    if (!read_file_to_buffer(path, &buffer)) {
        return false;
    }
    if (!kvm.copy_to_guest(mapping, guest_addr, buffer.data(), static_cast<std::uintptr_t>(buffer.size()), label)) {
        return false;
    }

    if (size_out != nullptr) {
        *size_out = static_cast<std::uintptr_t>(buffer.size());
    }
    return true;
}

std::vector<std::uint8_t> build_guest_dtb(const Bus &bus, std::uintptr_t memory_size, const std::string &bootargs,
                                          const std::optional<DtbRange> &initrd) {
    DtbConfig dtb = {};
    dtb.memory_base = GUEST_MEMORY_BASE;
    dtb.memory_size = memory_size;
    dtb.bootargs = bootargs;
    dtb.stdout_path = dtb_node_name("/soc/serial", UART0_BASE);
    dtb.initrd = initrd;
    return bus.build_dtb(dtb);
}

} // namespace

bool parse_uintptr_arg(const char *text, std::uintptr_t *value) {
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

void print_usage(const char *argv0) {
    std::fprintf(stderr,
                 "Usage:\n"
                 "  %s [kernel.bin]\n"
                 "  %s [-kernel PATH] [-initrd PATH] [-dtb PATH] [-append STRING] [--memory-size BYTES]\n",
                 argv0, argv0);
}

bool prepare_guest(const Kvm &kvm, const GuestMapping &mapping, const GuestBootOptions &options, const Bus &bus,
                   GuestEntry *entry) {
    if (!load_file_to_guest(kvm, options.kernel_path, mapping, GUEST_KERNEL_LOAD_ADDR, options.kernel_path)) {
        return false;
    }

    std::optional<DtbRange> initrd;
    if (options.initrd_path != nullptr) {
        std::uintptr_t initrd_size = 0;
        if (!load_file_to_guest(kvm, options.initrd_path, mapping, GUEST_INITRD_LOAD_ADDR, options.initrd_path,
                                &initrd_size)) {
            return false;
        }
        initrd = DtbRange{GUEST_INITRD_LOAD_ADDR, GUEST_INITRD_LOAD_ADDR + initrd_size};
    }

    std::vector<std::uint8_t> dtb_blob;
    if (options.dtb_path != nullptr) {
        if (!read_file_to_buffer(options.dtb_path, &dtb_blob)) {
            return false;
        }
    } else {
        dtb_blob = build_guest_dtb(bus, options.memory_size, options.bootargs, initrd);
    }

    if (!kvm.copy_to_guest(mapping, GUEST_DTB_LOAD_ADDR, dtb_blob.data(), static_cast<std::uintptr_t>(dtb_blob.size()),
                           options.dtb_path != nullptr ? options.dtb_path : "built-in dtb")) {
        return false;
    }

    *entry = {GUEST_KERNEL_LOAD_ADDR, GUEST_DTB_LOAD_ADDR};
    return true;
}

int boot_guest(const GuestMapping &mapping, const GuestEntry &entry, Kvm *kvm) {
    KvmCpu cpu;
    if (!kvm->create_cpu(&cpu)) {
        std::fprintf(stderr, "failed to create kvm vcpu\n");
        return 1;
    }

    std::printf("kvm host test ready: /dev/kvm fd=%d, vcpu fd=%d\n", kvm->raw_fd(), cpu.raw_fd());
    std::printf("mapped guest memory: guest=[0x%lx,0x%lx) host=%p\n", static_cast<unsigned long>(mapping.guest_base),
                static_cast<unsigned long>(mapping.guest_base + mapping.guest_size), mapping.host_base);

    if (!cpu.init(entry.pc, entry.a1)) {
        std::fprintf(stderr, "failed to initialize vcpu: pc=0x%lx a1=0x%lx\n", static_cast<unsigned long>(entry.pc),
                     static_cast<unsigned long>(entry.a1));
        return 1;
    }

    if (!cpu.run(nullptr)) {
        std::fprintf(stderr, "kvm vcpu run failed\n");
        return 1;
    }
    return 0;
}
} // namespace kvm_host
