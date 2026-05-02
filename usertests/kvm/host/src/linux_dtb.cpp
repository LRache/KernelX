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

} // namespace

std::uint32_t DtbBuilder::StringTable::add(const std::string &name) {
    const auto found = this->offsets_.find(name);
    if (found != this->offsets_.end()) {
        return found->second;
    }

    const std::uint32_t offset = static_cast<std::uint32_t>(this->data_.size());
    this->offsets_.emplace(name, offset);
    this->data_.insert(this->data_.end(), name.begin(), name.end());
    this->data_.push_back('\0');
    return offset;
}

const std::vector<std::uint8_t> &DtbBuilder::StringTable::data() const {
    return this->data_;
}

void DtbBuilder::begin_node(const std::string &name) {
    this->push_u32(FDT_BEGIN_NODE);
    this->structure_.insert(this->structure_.end(), name.begin(), name.end());
    this->structure_.push_back('\0');
    this->align_structure();
}

void DtbBuilder::end_node() {
    this->push_u32(FDT_END_NODE);
}

void DtbBuilder::prop_string(const std::string &name, const std::string &value) {
    std::vector<std::uint8_t> data(value.begin(), value.end());
    data.push_back('\0');
    this->prop_raw(name, data);
}

void DtbBuilder::prop_string_list(const std::string &name, const std::vector<std::string> &values) {
    std::vector<std::uint8_t> data;
    for (const std::string &value : values) {
        data.insert(data.end(), value.begin(), value.end());
        data.push_back('\0');
    }
    this->prop_raw(name, data);
}

void DtbBuilder::prop_u32(const std::string &name, std::uint32_t value) {
    std::vector<std::uint8_t> data;
    this->append_be32(&data, value);
    this->prop_raw(name, data);
}

void DtbBuilder::prop_u64(const std::string &name, std::uint64_t value) {
    std::vector<std::uint8_t> data;
    this->append_be64(&data, value);
    this->prop_raw(name, data);
}

void DtbBuilder::prop_cells(const std::string &name, const std::vector<std::uint32_t> &cells) {
    std::vector<std::uint8_t> data;
    for (std::uint32_t cell : cells) {
        this->append_be32(&data, cell);
    }
    this->prop_raw(name, data);
}

void DtbBuilder::prop_bool(const std::string &name) {
    this->prop_raw(name, {});
}

std::vector<std::uint8_t> DtbBuilder::finish_blob() {
    this->push_u32(FDT_END);
    this->align_structure();

    std::vector<std::uint8_t> blob;
    blob.resize(40);

    std::vector<std::uint8_t> reserve_map;
    this->append_be64(&reserve_map, 0);
    this->append_be64(&reserve_map, 0);

    const std::uint32_t off_mem_rsvmap = static_cast<std::uint32_t>(blob.size());
    blob.insert(blob.end(), reserve_map.begin(), reserve_map.end());

    const std::uint32_t off_dt_struct = static_cast<std::uint32_t>(blob.size());
    blob.insert(blob.end(), this->structure_.begin(), this->structure_.end());

    const std::uint32_t off_dt_strings = static_cast<std::uint32_t>(blob.size());
    const std::vector<std::uint8_t> &strings = this->strings_.data();
    blob.insert(blob.end(), strings.begin(), strings.end());

    this->write_header(&blob, 0, FDT_MAGIC);
    this->write_header(&blob, 4, static_cast<std::uint32_t>(blob.size()));
    this->write_header(&blob, 8, off_dt_struct);
    this->write_header(&blob, 12, off_dt_strings);
    this->write_header(&blob, 16, off_mem_rsvmap);
    this->write_header(&blob, 20, FDT_VERSION);
    this->write_header(&blob, 24, FDT_LAST_COMP_VERSION);
    this->write_header(&blob, 28, 0);
    this->write_header(&blob, 32, static_cast<std::uint32_t>(strings.size()));
    this->write_header(&blob, 36, static_cast<std::uint32_t>(this->structure_.size()));

    return blob;
}

void DtbBuilder::prop_raw(const std::string &name, const std::vector<std::uint8_t> &data) {
    this->push_u32(FDT_PROP);
    this->push_u32(static_cast<std::uint32_t>(data.size()));
    this->push_u32(this->strings_.add(name));
    this->structure_.insert(this->structure_.end(), data.begin(), data.end());
    this->align_structure();
}

void DtbBuilder::push_u32(std::uint32_t value) {
    this->append_be32(&this->structure_, value);
}

void DtbBuilder::align_structure() {
    while ((this->structure_.size() & 0x3u) != 0) {
        this->structure_.push_back(0);
    }
}

void DtbBuilder::append_be32(std::vector<std::uint8_t> *data, std::uint32_t value) {
    const std::uint32_t be = bswap32(value);
    const auto *bytes = reinterpret_cast<const std::uint8_t *>(&be);
    data->insert(data->end(), bytes, bytes + sizeof(be));
}

void DtbBuilder::append_be64(std::vector<std::uint8_t> *data, std::uint64_t value) {
    const std::uint64_t be = bswap64(value);
    const auto *bytes = reinterpret_cast<const std::uint8_t *>(&be);
    data->insert(data->end(), bytes, bytes + sizeof(be));
}

void DtbBuilder::write_header(std::vector<std::uint8_t> *blob, std::size_t offset, std::uint32_t value) {
    const std::uint32_t be = bswap32(value);
    std::memcpy(blob->data() + offset, &be, sizeof(be));
}

std::vector<std::uint32_t> dtb_reg_cells(std::uintptr_t addr, std::uintptr_t size) {
    return {
        static_cast<std::uint32_t>(addr >> 32),
        static_cast<std::uint32_t>(addr & 0xffffffffu),
        static_cast<std::uint32_t>(size >> 32),
        static_cast<std::uint32_t>(size & 0xffffffffu),
    };
}

std::string dtb_node_name(const char *prefix, std::uintptr_t addr) {
    char buffer[64] = {};
    std::snprintf(buffer, sizeof(buffer), "%s@%lx", prefix, static_cast<unsigned long>(addr));
    return buffer;
}

void DtbBuilder::config_dtb(const DtbConfig &config) {
    this->begin_node("");
    this->prop_u32("#address-cells", 2);
    this->prop_u32("#size-cells", 2);
    this->prop_string_list("compatible", {"kernelx,kvm-guest", "riscv-virtio"});
    this->prop_string("model", "KernelX KVM Guest");

    this->begin_node("chosen");
    if (!config.bootargs.empty()) {
        this->prop_string("bootargs", config.bootargs);
    }
    if (!config.stdout_path.empty()) {
        this->prop_string("stdout-path", config.stdout_path);
    }
    if (config.initrd.has_value()) {
        this->prop_u64("linux,initrd-start", config.initrd->start);
        this->prop_u64("linux,initrd-end", config.initrd->end);
    }
    this->end_node();

    this->begin_node(dtb_node_name("memory", config.memory_base));
    this->prop_string("device_type", "memory");
    this->prop_cells("reg", dtb_reg_cells(config.memory_base, config.memory_size));
    this->end_node();

    this->begin_node("cpus");
    this->prop_u32("#address-cells", 1);
    this->prop_u32("#size-cells", 0);
    this->prop_u32("timebase-frequency", config.timebase_frequency);

    this->begin_node("cpu@0");
    this->prop_string("device_type", "cpu");
    this->prop_u32("reg", 0);
    this->prop_string("status", "okay");
    this->prop_string("compatible", "riscv");
    this->prop_string("riscv,isa", config.riscv_isa);
    this->prop_string("mmu-type", config.mmu_type);

    this->begin_node("interrupt-controller");
    this->prop_u32("#interrupt-cells", 1);
    this->prop_bool("interrupt-controller");
    this->prop_string("compatible", "riscv,cpu-intc");
    this->prop_u32("phandle", config.cpu_intc_phandle);
    this->end_node();

    this->end_node();
    this->end_node();

    this->begin_node("soc");
    this->prop_u32("#address-cells", 2);
    this->prop_u32("#size-cells", 2);
    this->prop_string("compatible", "simple-bus");
    this->prop_bool("ranges");
}

std::vector<std::uint8_t> DtbBuilder::finish_dtb() {
    this->end_node();
    this->end_node();

    return this->finish_blob();
}

std::vector<std::uint8_t> build_dtb(const DtbConfig &config) {
    DtbBuilder builder;
    builder.config_dtb(config);
    return builder.finish_dtb();
}
} // namespace kvm_host
