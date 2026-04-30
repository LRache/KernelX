#include "linux_dtb.hpp"

#include <cstdio>
#include <cstdint>
#include <cstring>
#include <map>
#include <string>
#include <utility>
#include <vector>

namespace kvm_host {
namespace {
constexpr std::uint32_t FDT_MAGIC = 0xd00dfeed;
constexpr std::uint32_t FDT_VERSION = 17;
constexpr std::uint32_t FDT_LAST_COMP_VERSION = 16;

constexpr std::uint32_t FDT_BEGIN_NODE = 0x1;
constexpr std::uint32_t FDT_END_NODE = 0x2;
constexpr std::uint32_t FDT_PROP = 0x3;
constexpr std::uint32_t FDT_END = 0x9;

static std::uint32_t bswap32(std::uint32_t value) {
    return ((value & 0x000000ffu) << 24) | ((value & 0x0000ff00u) << 8) | ((value & 0x00ff0000u) >> 8) |
           ((value & 0xff000000u) >> 24);
}

static std::uint64_t bswap64(std::uint64_t value) {
    return (static_cast<std::uint64_t>(bswap32(static_cast<std::uint32_t>(value))) << 32) |
           bswap32(static_cast<std::uint32_t>(value >> 32));
}

class StringTable {
public:
    std::uint32_t add(const std::string &name) {
        const auto found = offsets_.find(name);
        if (found != offsets_.end()) {
            return found->second;
        }

        const std::uint32_t offset = static_cast<std::uint32_t>(data_.size());
        offsets_.emplace(name, offset);
        data_.insert(data_.end(), name.begin(), name.end());
        data_.push_back('\0');
        return offset;
    }

    const std::vector<std::uint8_t> &data() const {
        return data_;
    }

private:
    std::map<std::string, std::uint32_t> offsets_;
    std::vector<std::uint8_t> data_;
};

class DtbBuilder {
public:
    void begin_node(const std::string &name) {
        push_u32(FDT_BEGIN_NODE);
        structure_.insert(structure_.end(), name.begin(), name.end());
        structure_.push_back('\0');
        align_structure();
    }

    void end_node() {
        push_u32(FDT_END_NODE);
    }

    void prop_string(const std::string &name, const std::string &value) {
        std::vector<std::uint8_t> data(value.begin(), value.end());
        data.push_back('\0');
        prop_raw(name, data);
    }

    void prop_string_list(const std::string &name, const std::vector<std::string> &values) {
        std::vector<std::uint8_t> data;
        for (const std::string &value : values) {
            data.insert(data.end(), value.begin(), value.end());
            data.push_back('\0');
        }
        prop_raw(name, data);
    }

    void prop_u32(const std::string &name, std::uint32_t value) {
        std::vector<std::uint8_t> data;
        append_be32(&data, value);
        prop_raw(name, data);
    }

    void prop_u64(const std::string &name, std::uint64_t value) {
        std::vector<std::uint8_t> data;
        append_be64(&data, value);
        prop_raw(name, data);
    }

    void prop_cells(const std::string &name, const std::vector<std::uint32_t> &cells) {
        std::vector<std::uint8_t> data;
        for (std::uint32_t cell : cells) {
            append_be32(&data, cell);
        }
        prop_raw(name, data);
    }

    void prop_bool(const std::string &name) {
        prop_raw(name, {});
    }

    std::vector<std::uint8_t> finish() {
        push_u32(FDT_END);
        align_structure();

        std::vector<std::uint8_t> blob;
        blob.resize(40);

        std::vector<std::uint8_t> reserve_map;
        append_be64(&reserve_map, 0);
        append_be64(&reserve_map, 0);

        const std::uint32_t off_mem_rsvmap = static_cast<std::uint32_t>(blob.size());
        blob.insert(blob.end(), reserve_map.begin(), reserve_map.end());

        const std::uint32_t off_dt_struct = static_cast<std::uint32_t>(blob.size());
        blob.insert(blob.end(), structure_.begin(), structure_.end());

        const std::uint32_t off_dt_strings = static_cast<std::uint32_t>(blob.size());
        const std::vector<std::uint8_t> &strings = strings_.data();
        blob.insert(blob.end(), strings.begin(), strings.end());

        write_header(&blob, 0, FDT_MAGIC);
        write_header(&blob, 4, static_cast<std::uint32_t>(blob.size()));
        write_header(&blob, 8, off_dt_struct);
        write_header(&blob, 12, off_dt_strings);
        write_header(&blob, 16, off_mem_rsvmap);
        write_header(&blob, 20, FDT_VERSION);
        write_header(&blob, 24, FDT_LAST_COMP_VERSION);
        write_header(&blob, 28, 0);
        write_header(&blob, 32, static_cast<std::uint32_t>(strings.size()));
        write_header(&blob, 36, static_cast<std::uint32_t>(structure_.size()));

        return blob;
    }

private:
    void prop_raw(const std::string &name, const std::vector<std::uint8_t> &data) {
        push_u32(FDT_PROP);
        push_u32(static_cast<std::uint32_t>(data.size()));
        push_u32(strings_.add(name));
        structure_.insert(structure_.end(), data.begin(), data.end());
        align_structure();
    }

    void push_u32(std::uint32_t value) {
        append_be32(&structure_, value);
    }

    void align_structure() {
        while ((structure_.size() & 0x3u) != 0) {
            structure_.push_back(0);
        }
    }

    static void append_be32(std::vector<std::uint8_t> *data, std::uint32_t value) {
        const std::uint32_t be = bswap32(value);
        const auto *bytes = reinterpret_cast<const std::uint8_t *>(&be);
        data->insert(data->end(), bytes, bytes + sizeof(be));
    }

    static void append_be64(std::vector<std::uint8_t> *data, std::uint64_t value) {
        const std::uint64_t be = bswap64(value);
        const auto *bytes = reinterpret_cast<const std::uint8_t *>(&be);
        data->insert(data->end(), bytes, bytes + sizeof(be));
    }

    static void write_header(std::vector<std::uint8_t> *blob, std::size_t offset, std::uint32_t value) {
        const std::uint32_t be = bswap32(value);
        std::memcpy(blob->data() + offset, &be, sizeof(be));
    }

    StringTable strings_;
    std::vector<std::uint8_t> structure_;
};

static std::vector<std::uint32_t> reg_cells(std::uintptr_t addr, std::uintptr_t size) {
    return {
        static_cast<std::uint32_t>(addr >> 32),
        static_cast<std::uint32_t>(addr & 0xffffffffu),
        static_cast<std::uint32_t>(size >> 32),
        static_cast<std::uint32_t>(size & 0xffffffffu),
    };
}

static std::string node_name(const char *prefix, std::uintptr_t addr) {
    char buffer[64] = {};
    std::snprintf(buffer, sizeof(buffer), "%s@%lx", prefix, static_cast<unsigned long>(addr));
    return buffer;
}
} // namespace

std::vector<std::uint8_t> build_linux_guest_dtb(const LinuxGuestDtbConfig &config) {
    DtbBuilder builder;

    builder.begin_node("");
    builder.prop_u32("#address-cells", 2);
    builder.prop_u32("#size-cells", 2);
    builder.prop_string_list("compatible", {"kernelx,kvm-linux-guest", "riscv-virtio"});
    builder.prop_string("model", "KernelX KVM Linux Guest");

    builder.begin_node("chosen");
    if (!config.bootargs.empty()) {
        builder.prop_string("bootargs", config.bootargs);
    }
    if (!config.stdout_path.empty()) {
        builder.prop_string("stdout-path", config.stdout_path);
    }
    if (config.has_initrd) {
        builder.prop_u64("linux,initrd-start", config.initrd_start);
        builder.prop_u64("linux,initrd-end", config.initrd_end);
    }
    builder.end_node();

    builder.begin_node(node_name("memory", config.memory_base));
    builder.prop_string("device_type", "memory");
    builder.prop_cells("reg", reg_cells(config.memory_base, config.memory_size));
    builder.end_node();

    builder.begin_node("cpus");
    builder.prop_u32("#address-cells", 1);
    builder.prop_u32("#size-cells", 0);
    builder.prop_u32("timebase-frequency", config.timebase_frequency);

    builder.begin_node("cpu@0");
    builder.prop_string("device_type", "cpu");
    builder.prop_u32("reg", 0);
    builder.prop_string("status", "okay");
    builder.prop_string("compatible", "riscv");
    builder.prop_string("riscv,isa", config.riscv_isa);
    builder.prop_string("mmu-type", config.mmu_type);

    builder.begin_node("interrupt-controller");
    builder.prop_u32("#interrupt-cells", 1);
    builder.prop_bool("interrupt-controller");
    builder.prop_string("compatible", "riscv,cpu-intc");
    builder.prop_u32("phandle", config.cpu_intc_phandle);
    builder.end_node();

    builder.end_node();
    builder.end_node();

    builder.begin_node("soc");
    builder.prop_u32("#address-cells", 2);
    builder.prop_u32("#size-cells", 2);
    builder.prop_string("compatible", "simple-bus");
    builder.prop_bool("ranges");

    builder.begin_node(node_name("serial", config.uart_base));
    builder.prop_string("compatible", "ns16550a");
    builder.prop_string("status", "okay");
    builder.prop_cells("reg", reg_cells(config.uart_base, config.uart_size));
    builder.prop_u32("clock-frequency", config.uart_clock_hz);
    builder.prop_u32("current-speed", config.uart_baud);
    builder.prop_u32("reg-shift", 0);
    builder.prop_u32("reg-io-width", 1);
    if (config.has_plic && config.uart_irq != 0) {
        builder.prop_u32("interrupt-parent", config.plic_phandle);
        builder.prop_u32("interrupts", config.uart_irq);
    }
    builder.end_node();

    if (config.has_plic) {
        builder.begin_node(node_name("plic", config.plic_base));
        builder.prop_u32("phandle", config.plic_phandle);
        builder.prop_u32("riscv,ndev", config.plic_ndev);
        builder.prop_cells("reg", reg_cells(config.plic_base, config.plic_size));
        builder.prop_cells("interrupts-extended", {config.cpu_intc_phandle, 11, config.cpu_intc_phandle, 9});
        builder.prop_bool("interrupt-controller");
        builder.prop_string_list("compatible", {"sifive,plic-1.0.0", "riscv,plic0"});
        builder.prop_u32("#address-cells", 0);
        builder.prop_u32("#interrupt-cells", 1);
        builder.end_node();
    }

    builder.end_node();
    builder.end_node();

    return builder.finish();
}
} // namespace kvm_host
