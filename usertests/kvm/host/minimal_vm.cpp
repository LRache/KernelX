#include <cstdint>
#include <fcntl.h>
#include <stdio.h>
#include <sys/ioctl.h>
#include <unistd.h>

namespace {
constexpr unsigned long KVM_CREATE_VCPU = 1;
constexpr unsigned long KVM_MAP_AREA = 2;
constexpr unsigned long KVM_GET_REGS = 2;
constexpr unsigned long KVM_SET_REGS = 3;

struct KvmMapArea {
    std::uintptr_t addr;
    std::uintptr_t length;
};

struct KvmRegs {
    std::uintptr_t pc;
    std::uintptr_t ra;
    std::uintptr_t sp;
    std::uintptr_t gp;
    std::uintptr_t tp;
    std::uintptr_t t0;
    std::uintptr_t t1;
    std::uintptr_t t2;
    std::uintptr_t s0;
    std::uintptr_t s1;
    std::uintptr_t a0;
    std::uintptr_t a1;
    std::uintptr_t a2;
    std::uintptr_t a3;
    std::uintptr_t a4;
    std::uintptr_t a5;
    std::uintptr_t a6;
    std::uintptr_t a7;
    std::uintptr_t s2;
    std::uintptr_t s3;
    std::uintptr_t s4;
    std::uintptr_t s5;
    std::uintptr_t s6;
    std::uintptr_t s7;
    std::uintptr_t s8;
    std::uintptr_t s9;
    std::uintptr_t s10;
    std::uintptr_t s11;
    std::uintptr_t t3;
    std::uintptr_t t4;
    std::uintptr_t t5;
    std::uintptr_t t6;
};
}

int main() {
    int kvm_fd = open("/dev/kvm", O_RDONLY);
    if (kvm_fd < 0) {
        perror("open /dev/kvm");
        return 1;
    }

    KvmMapArea map_area = {};
    map_area.addr = 0x80000000;
    map_area.length = 0x2000;
    if (ioctl(kvm_fd, KVM_MAP_AREA, &map_area) < 0) {
        perror("ioctl(KVM_MAP_AREA)");
        close(kvm_fd);
        return 1;
    }

    int vcpu_fd = ioctl(kvm_fd, KVM_CREATE_VCPU, 0);
    if (vcpu_fd < 0) {
        perror("ioctl(KVM_CREATE_VCPU)");
        close(kvm_fd);
        return 1;
    }

    printf("kvm host test ready: /dev/kvm fd=%d, vcpu fd=%d\n", kvm_fd, vcpu_fd);

    KvmRegs regs = {};
    if (ioctl(vcpu_fd, KVM_GET_REGS, &regs) < 0) {
        perror("ioctl(KVM_GET_REGS)");
        close(vcpu_fd);
        close(kvm_fd);
        return 1;
    }

    regs.pc = 0x80000000;
    regs.a0 = 0x1234;
    if (ioctl(vcpu_fd, KVM_SET_REGS, &regs) < 0) {
        perror("ioctl(KVM_SET_REGS)");
        close(vcpu_fd);
        close(kvm_fd);
        return 1;
    }

    KvmRegs verify = {};
    if (ioctl(vcpu_fd, KVM_GET_REGS, &verify) < 0) {
        perror("ioctl(KVM_GET_REGS verify)");
        close(vcpu_fd);
        close(kvm_fd);
        return 1;
    }

    printf(
        "vcpu pc=0x%lx a0=0x%lx after set_regs\n",
        static_cast<unsigned long>(verify.pc),
        static_cast<unsigned long>(verify.a0)
    );

    close(vcpu_fd);
    close(kvm_fd);
    return 0;
}
