// SPDX-License-Identifier: MIT
// Portions of this file are adapted from KXemu's UART device code.
// Original project: KXemu (MIT License), Copyright (c) 2024 HD-CSKX.
// Source repository: git@github.com:HD-CSKX/KXemu.git

#include "device/uart.hpp"

#include "linux_dtb.hpp"

#include <arpa/inet.h>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <iostream>
#include <limits>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

namespace kvm_host {
static constexpr std::uint8_t UART_DLAB = 1u << 7;
static constexpr std::uint8_t UART_LSR_DATA_READY = 1u << 0;
static constexpr std::uint8_t UART_LSR_THR_EMPTY = 1u << 5;
static constexpr std::uint8_t UART_LSR_TRANSMITTER_EMPTY = 1u << 6;
static constexpr std::uint8_t UART_IER_RX_AVAILABLE = 1u << 0;
static constexpr std::uint8_t UART_IER_THR_EMPTY = 1u << 1;
static constexpr std::uint8_t UART_IIR_NONE = 0b11000001;
static constexpr std::uint8_t UART_IIR_THR_EMPTY = 0b11000010;
static constexpr std::uint8_t UART_IIR_RX_AVAILABLE = 0b11000100;
static constexpr std::size_t UART_FIFO_CAPACITY = 1024;

static constexpr std::uintptr_t UART_RBR_THR_DLL = 0;
static constexpr std::uintptr_t UART_IER_DLM = 1;
static constexpr std::uintptr_t UART_IIR_FCR = 2;
static constexpr std::uintptr_t UART_LCR = 3;
static constexpr std::uintptr_t UART_MCR = 4;
static constexpr std::uintptr_t UART_LSR = 5;
static constexpr std::uintptr_t UART_MSR = 6;

static int open_socket_client(const std::string &ip, int port) {
    const int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0) {
        std::fprintf(stderr, "uart socket: create failed: %s\n", std::strerror(errno));
        return -1;
    }

    sockaddr_in server = {};
    server.sin_family = AF_INET;
    server.sin_port = htons(static_cast<std::uint16_t>(port));
    if (inet_pton(AF_INET, ip.c_str(), &server.sin_addr) <= 0) {
        std::fprintf(stderr, "uart socket: invalid address %s\n", ip.c_str());
        close(sockfd);
        return -1;
    }

    if (connect(sockfd, reinterpret_cast<const sockaddr *>(&server), sizeof(server)) < 0) {
        std::fprintf(stderr, "uart socket: connect %s:%d failed: %s\n", ip.c_str(), port, std::strerror(errno));
        close(sockfd);
        return -1;
    }

    return sockfd;
}

void Uart16650Device::refresh_interrupt_state() {
    if (!this->queue_.empty() && (this->ier_ & UART_IER_RX_AVAILABLE) != 0) {
        this->iir_ = UART_IIR_RX_AVAILABLE;
        this->interrupt_ = true;
        return;
    }

    if (this->thr_interrupt_pending_ && (this->ier_ & UART_IER_THR_EMPTY) != 0) {
        this->iir_ = UART_IIR_THR_EMPTY;
        this->interrupt_ = true;
        return;
    }

    this->iir_ = UART_IIR_NONE;
    this->interrupt_ = false;
}

bool Uart16650Device::read(std::uintptr_t offset, std::size_t width, std::uint64_t *value) {
    if (value == nullptr || width != 1) {
        return false;
    }

    switch (offset) {
        case UART_RBR_THR_DLL:
            if ((this->lcr_ & UART_DLAB) != 0) {
                *value = this->lsb_;
                return true;
            }

            {
                std::lock_guard<std::mutex> lock(this->queue_mutex_);
                if (this->queue_.empty()) {
                    *value = std::numeric_limits<std::uint64_t>::max();
                    return true;
                }

                const std::uint8_t c = this->queue_.front();
                this->queue_.pop();
                if (this->queue_.empty()) {
                    this->lsr_ &= static_cast<std::uint8_t>(~UART_LSR_DATA_READY);
                }
                this->refresh_interrupt_state();

                *value = c;
                return true;
            }
        case UART_IER_DLM:
            *value = (this->lcr_ & UART_DLAB) != 0 ? this->msb_ : this->ier_;
            return true;
        case UART_IIR_FCR: {
            const std::uint8_t iir = this->iir_;
            *value = iir;
            if ((iir & 0x0f) == (UART_IIR_THR_EMPTY & 0x0f)) {
                this->thr_interrupt_pending_ = false;
                this->refresh_interrupt_state();
            }
            return true;
        }
        case UART_LCR:
            *value = this->lcr_;
            return true;
        case UART_LSR:
            *value = this->lsr_;
            return true;
        case UART_MSR:
            *value = this->msr_;
            return true;
        default:
            *value = 0;
            return true;
    }
}

bool Uart16650Device::write(std::uintptr_t offset, std::size_t width, std::uint64_t value) {
    if (width != 1) {
        return false;
    }

    const std::uint8_t byte = static_cast<std::uint8_t>(value);
    switch (offset) {
        case UART_RBR_THR_DLL:
            if ((this->lcr_ & UART_DLAB) != 0) {
                this->lsb_ = byte;
                return true;
            }

            this->send_byte(byte);
            if ((this->lsr_ & UART_LSR_THR_EMPTY) != 0 && (this->ier_ & UART_IER_THR_EMPTY) != 0) {
                this->thr_interrupt_pending_ = true;
            }
            this->refresh_interrupt_state();
            return true;
        case UART_IER_DLM: {
            if ((this->lcr_ & UART_DLAB) != 0) {
                this->msb_ = byte;
                return true;
            }

            const std::uint8_t old_ier = this->ier_;
            this->ier_ = byte;
            if ((old_ier & UART_IER_THR_EMPTY) == 0 && (this->ier_ & UART_IER_THR_EMPTY) != 0 &&
                (this->lsr_ & UART_LSR_THR_EMPTY) != 0) {
                this->thr_interrupt_pending_ = true;
            }
            this->refresh_interrupt_state();
            return true;
        }
        case UART_IIR_FCR:
            if ((byte & (1u << 1)) != 0) {
                std::lock_guard<std::mutex> lock(this->queue_mutex_);
                while (!this->queue_.empty()) {
                    this->queue_.pop();
                }
                this->lsr_ &= static_cast<std::uint8_t>(~UART_LSR_DATA_READY);
            }

            switch ((byte >> 6) & 0x3) {
                case 0:
                    this->recv_fifo_trigger_byte_count_ = 1;
                    break;
                case 1:
                    this->recv_fifo_trigger_byte_count_ = 4;
                    break;
                case 2:
                    this->recv_fifo_trigger_byte_count_ = 8;
                    break;
                case 3:
                    this->recv_fifo_trigger_byte_count_ = 14;
                    break;
            }
            this->refresh_interrupt_state();
            return true;
        case UART_LCR:
            this->lcr_ = byte;
            return true;
        case UART_MCR:
            return true;
        default:
            return true;
    }
}

void Uart16650Device::update() {
    int recv_fd = -1;
    if (this->mode_ == Mode::Socket) {
        recv_fd = this->recv_socket_;
    } else if (this->mode_ == Mode::Stream && !this->stream_input_closed_) {
        recv_fd = STDIN_FILENO;
    }
    if (recv_fd < 0) {
        return;
    }

    timeval timeout = {};
    fd_set read_fds;
    FD_ZERO(&read_fds);
    FD_SET(recv_fd, &read_fds);

    const int ready = select(recv_fd + 1, &read_fds, nullptr, nullptr, &timeout);
    if (ready <= 0) {
        return;
    }

    char buffer[64];
    const ssize_t bytes = ::read(recv_fd, buffer, sizeof(buffer));
    if (bytes <= 0) {
        if (this->mode_ == Mode::Socket) {
            std::fprintf(stderr, "uart socket: receive failed: %s\n",
                         bytes < 0 ? std::strerror(errno) : "peer closed");
            close(this->recv_socket_);
            this->recv_socket_ = -1;
            this->mode_ = Mode::None;
        } else if (bytes < 0 && errno != EINTR && errno != EAGAIN && errno != EWOULDBLOCK) {
            this->stream_input_closed_ = true;
        }
        return;
    }

    for (ssize_t i = 0; i < bytes; i++) {
        this->recv_byte(static_cast<std::uint8_t>(buffer[i]));
    }
}

bool Uart16650Device::interrupt_pending() {
    return this->interrupt_;
}

void Uart16650Device::clear_interrupt() {
    this->refresh_interrupt_state();
}

bool Uart16650Device::putch(std::uint8_t data) {
    std::lock_guard<std::mutex> lock(this->queue_mutex_);
    if (this->queue_.size() >= UART_FIFO_CAPACITY) {
        return false;
    }

    this->queue_.push(data);
    this->lsr_ |= UART_LSR_DATA_READY;
    this->refresh_interrupt_state();
    return true;
}

void Uart16650Device::set_output_stream(std::ostream &os) {
    if (this->mode_ == Mode::Socket) {
        std::fprintf(stderr, "uart stream: cannot switch output backend from socket to stream\n");
        return;
    }

    this->mode_ = Mode::Stream;
    this->stream_ = &os;
    this->lsr_ |= static_cast<std::uint8_t>(UART_LSR_THR_EMPTY | UART_LSR_TRANSMITTER_EMPTY);
}

bool Uart16650Device::open_socket(const std::string &ip, int port) {
    if (this->mode_ != Mode::None) {
        std::fprintf(stderr, "uart socket: device is already bound to an output backend\n");
        return false;
    }

    this->recv_socket_ = open_socket_client(ip, port);
    this->send_socket_ = open_socket_client(ip, port + 1);
    if (this->recv_socket_ < 0 || this->send_socket_ < 0) {
        if (this->recv_socket_ >= 0) {
            close(this->recv_socket_);
            this->recv_socket_ = -1;
        }
        if (this->send_socket_ >= 0) {
            close(this->send_socket_);
            this->send_socket_ = -1;
        }
        return false;
    }

    this->mode_ = Mode::Socket;
    this->lsr_ |= static_cast<std::uint8_t>(UART_LSR_THR_EMPTY | UART_LSR_TRANSMITTER_EMPTY);
    return true;
}

void Uart16650Device::recv_byte(std::uint8_t c) {
    std::lock_guard<std::mutex> lock(this->queue_mutex_);
    if (this->queue_.size() >= UART_FIFO_CAPACITY) {
        std::fprintf(stderr, "uart receive queue is full, dropping byte\n");
        return;
    }

    this->queue_.push(c);
    this->lsr_ |= UART_LSR_DATA_READY;
    this->refresh_interrupt_state();
}

void Uart16650Device::send_byte(std::uint8_t c) {
    std::lock_guard<std::mutex> lock(this->sender_mutex_);

    switch (this->mode_) {
        case Mode::Stream:
            if (this->stream_ == nullptr) {
                std::cout << c;
                std::cout.flush();
            } else {
                *this->stream_ << c;
                this->stream_->flush();
            }
            return;
        case Mode::Socket:
            if (this->send_socket_ < 0) {
                return;
            }
            if (send(this->send_socket_, &c, 1, 0) <= 0) {
                std::fprintf(stderr, "uart socket: send failed: %s\n", std::strerror(errno));
            }
            return;
        case Mode::None:
            std::cout << c;
            std::cout.flush();
            return;
    }
}

void Uart16650Device::config_dtb(DtbBuilder &builder, const DtbConfig &config,
                                          std::uintptr_t guest_addr, std::uintptr_t length, unsigned int id) const {
    builder.begin_node(dtb_node_name("serial", guest_addr));
    builder.prop_string("compatible", "ns16550a");
    builder.prop_string("status", "okay");
    builder.prop_cells("reg", dtb_reg_cells(guest_addr, length));
    builder.prop_u32("clock-frequency", 3686400);
    builder.prop_u32("current-speed", 115200);
    builder.prop_u32("reg-shift", 0);
    builder.prop_u32("reg-io-width", 1);
    if (id != 0) {
        builder.prop_u32("interrupt-parent", config.plic_phandle);
        builder.prop_u32("interrupts", static_cast<std::uint32_t>(id));
    }
    builder.end_node();
}

const char *Uart16650Device::type_name() const {
    return "uart16650";
}

Uart16650Device::~Uart16650Device() {
    if (this->recv_socket_ >= 0) {
        close(this->recv_socket_);
    }
    if (this->send_socket_ >= 0) {
        close(this->send_socket_);
    }
}
} // namespace kvm_host
