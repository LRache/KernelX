#pragma once

#include <cstddef>
#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <vector>

namespace kvm_host {
struct DtbConfig;

class DtbBuilder {
public:
    void begin_node(const std::string &name);
    void end_node();
    void prop_string(const std::string &name, const std::string &value);
    void prop_string_list(const std::string &name, const std::vector<std::string> &values);
    void prop_u32(const std::string &name, std::uint32_t value);
    void prop_u64(const std::string &name, std::uint64_t value);
    void prop_cells(const std::string &name, const std::vector<std::uint32_t> &cells);
    void prop_bool(const std::string &name);
    void config_dtb(const DtbConfig &config);
    std::vector<std::uint8_t> finish_dtb();

private:
    class StringTable {
    public:
        std::uint32_t add(const std::string &name);
        const std::vector<std::uint8_t> &data() const;

    private:
        std::map<std::string, std::uint32_t> offsets_;
        std::vector<std::uint8_t> data_;
    };

    void prop_raw(const std::string &name, const std::vector<std::uint8_t> &data);
    std::vector<std::uint8_t> finish_blob();
    void push_u32(std::uint32_t value);
    void align_structure();
    static void append_be32(std::vector<std::uint8_t> *data, std::uint32_t value);
    static void append_be64(std::vector<std::uint8_t> *data, std::uint64_t value);
    static void write_header(std::vector<std::uint8_t> *blob, std::size_t offset, std::uint32_t value);

    StringTable strings_;
    std::vector<std::uint8_t> structure_;
};

struct DtbRange {
    std::uintptr_t start = 0;
    std::uintptr_t end = 0;
};

struct DtbConfig {
    std::uintptr_t memory_base = 0;
    std::uintptr_t memory_size = 0;
    std::uint32_t timebase_frequency = 10000000;
    std::uint32_t cpu_intc_phandle = 1;
    std::uint32_t plic_phandle = 2;
    std::optional<DtbRange> initrd;
    std::string bootargs;
    std::string stdout_path;
    std::string riscv_isa = "rv64imafd_zicsr_zifencei";
    std::string mmu_type = "riscv,sv39";
};

std::vector<std::uint32_t> dtb_reg_cells(std::uintptr_t addr, std::uintptr_t size);
std::string dtb_node_name(const char *prefix, std::uintptr_t addr);
std::vector<std::uint8_t> build_dtb(const DtbConfig &config);
} // namespace kvm_host
