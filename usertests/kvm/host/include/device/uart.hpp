// SPDX-License-Identifier: MIT
// Portions of this file are adapted from KXemu's UART device code.
// Original project: KXemu (MIT License), Copyright (c) 2024 HD-CSKX.
// Source repository: git@github.com:HD-CSKX/KXemu.git

#pragma once

#include "device/mmio.hpp"

#include <cstddef>
#include <cstdint>
#include <iosfwd>
#include <mutex>
#include <queue>
#include <string>

namespace kvm_host {
class Uart16650Device final : public MmioDevice {
public:
    static constexpr std::uintptr_t kLength = 8;

    ~Uart16650Device() override;

    bool read(std::uintptr_t offset, std::size_t width, std::uint64_t *value) override;
    bool write(std::uintptr_t offset, std::size_t width, std::uint64_t value) override;
    void update() override;
    bool interrupt_pending() override;
    void clear_interrupt() override;
    void config_dtb(DtbBuilder &builder, const DtbConfig &config, std::uintptr_t guest_addr,
                             std::uintptr_t length, unsigned int id) const override;
    const char *type_name() const override;

    bool putch(std::uint8_t data);
    void set_output_stream(std::ostream &os);
    bool open_socket(const std::string &ip, int port);

private:
    enum class Mode {
        None,
        Stream,
        Socket,
    };

    void refresh_interrupt_state();
    void recv_byte(std::uint8_t c);
    void send_byte(std::uint8_t c);

    Mode mode_ = Mode::None;
    std::ostream *stream_ = nullptr;

    int send_socket_ = -1;
    int recv_socket_ = -1;

    std::mutex queue_mutex_;
    std::queue<std::uint8_t> queue_;
    std::mutex sender_mutex_;

    std::uint8_t lsb_ = 0x00;
    std::uint8_t msb_ = 0x00;
    std::uint8_t ier_ = 0b00000000;
    std::uint8_t iir_ = 0b11000001;
    std::uint8_t lcr_ = 0b00000011;
    std::uint8_t lsr_ = 0b01100000;
    std::uint8_t msr_ = 0x00;

    bool interrupt_ = false;
    bool thr_interrupt_pending_ = false;
    bool stream_input_closed_ = false;
    unsigned int recv_fifo_trigger_byte_count_ = 1;
};

using MmioConsoleDevice = Uart16650Device;
} // namespace kvm_host
