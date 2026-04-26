// SPDX-License-Identifier: MIT
// Portions of this file are adapted from KXemu's UART device code.
// Original project: KXemu (MIT License), Copyright (c) 2024 HD-CSKX.
// Source repository: git@github.com:HD-CSKX/KXemu.git

#include "uart.hpp"

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

bool Uart16650Device::read(std::uintptr_t offset, std::size_t width, std::uint64_t *value) {
    if (value == nullptr || width != 1) {
        return false;
    }

    switch (offset) {
        case UART_RBR_THR_DLL:
            if ((lcr_ & UART_DLAB) != 0) {
                *value = lsb_;
                return true;
            }

            {
                std::lock_guard<std::mutex> lock(queue_mutex_);
                if (queue_.empty()) {
                    *value = std::numeric_limits<std::uint64_t>::max();
                    return true;
                }

                const std::uint8_t c = queue_.front();
                queue_.pop();
                if (queue_.empty()) {
                    lsr_ &= static_cast<std::uint8_t>(~UART_LSR_DATA_READY);
                }
                if ((ier_ & UART_IER_RX_AVAILABLE) != 0 && (iir_ & 0x3f) == 0x02) {
                    iir_ = 0b11000001;
                    interrupt_ = false;
                }

                *value = c;
                return true;
            }
        case UART_IER_DLM:
            *value = (lcr_ & UART_DLAB) != 0 ? msb_ : ier_;
            return true;
        case UART_IIR_FCR:
            *value = iir_;
            return true;
        case UART_LCR:
            *value = lcr_;
            return true;
        case UART_LSR:
            *value = lsr_;
            return true;
        case UART_MSR:
            *value = msr_;
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
            if ((lcr_ & UART_DLAB) != 0) {
                lsb_ = byte;
                return true;
            }

            send_byte(byte);
            return true;
        case UART_IER_DLM:
            if ((lcr_ & UART_DLAB) != 0) {
                msb_ = byte;
                return true;
            }

            ier_ = byte;
            return true;
        case UART_IIR_FCR:
            if ((byte & (1u << 1)) != 0) {
                std::lock_guard<std::mutex> lock(queue_mutex_);
                while (!queue_.empty()) {
                    queue_.pop();
                }
                lsr_ &= static_cast<std::uint8_t>(~UART_LSR_DATA_READY);
            }

            switch ((byte >> 6) & 0x3) {
                case 0:
                    recv_fifo_trigger_byte_count_ = 1;
                    break;
                case 1:
                    recv_fifo_trigger_byte_count_ = 4;
                    break;
                case 2:
                    recv_fifo_trigger_byte_count_ = 8;
                    break;
                case 3:
                    recv_fifo_trigger_byte_count_ = 14;
                    break;
            }
            return true;
        case UART_LCR:
            lcr_ = byte;
            return true;
        case UART_MCR:
            return true;
        default:
            return true;
    }
}

void Uart16650Device::update() {
    if (mode_ != Mode::Socket || recv_socket_ < 0) {
        return;
    }

    timeval timeout = {};
    fd_set read_fds;
    FD_ZERO(&read_fds);
    FD_SET(recv_socket_, &read_fds);

    const int ready = select(recv_socket_ + 1, &read_fds, nullptr, nullptr, &timeout);
    if (ready <= 0) {
        return;
    }

    char buffer[64];
    const ssize_t bytes = ::read(recv_socket_, buffer, sizeof(buffer));
    if (bytes <= 0) {
        std::fprintf(stderr, "uart socket: receive failed: %s\n", bytes < 0 ? std::strerror(errno) : "peer closed");
        close(recv_socket_);
        recv_socket_ = -1;
        mode_ = Mode::None;
        return;
    }

    for (ssize_t i = 0; i < bytes; i++) {
        recv_byte(static_cast<std::uint8_t>(buffer[i]));
    }
}

bool Uart16650Device::interrupt_pending() {
    return interrupt_;
}

void Uart16650Device::clear_interrupt() {
    interrupt_ = false;
}

bool Uart16650Device::putch(std::uint8_t data) {
    std::lock_guard<std::mutex> lock(queue_mutex_);
    if (queue_.size() >= UART_FIFO_CAPACITY) {
        return false;
    }

    queue_.push(data);
    lsr_ |= UART_LSR_DATA_READY;
    return true;
}

void Uart16650Device::set_output_stream(std::ostream &os) {
    if (mode_ == Mode::Socket) {
        std::fprintf(stderr, "uart stream: cannot switch output backend from socket to stream\n");
        return;
    }

    mode_ = Mode::Stream;
    stream_ = &os;
    lsr_ |= static_cast<std::uint8_t>(UART_LSR_THR_EMPTY | UART_LSR_TRANSMITTER_EMPTY);
}

bool Uart16650Device::open_socket(const std::string &ip, int port) {
    if (mode_ != Mode::None) {
        std::fprintf(stderr, "uart socket: device is already bound to an output backend\n");
        return false;
    }

    recv_socket_ = open_socket_client(ip, port);
    send_socket_ = open_socket_client(ip, port + 1);
    if (recv_socket_ < 0 || send_socket_ < 0) {
        if (recv_socket_ >= 0) {
            close(recv_socket_);
            recv_socket_ = -1;
        }
        if (send_socket_ >= 0) {
            close(send_socket_);
            send_socket_ = -1;
        }
        return false;
    }

    mode_ = Mode::Socket;
    lsr_ |= static_cast<std::uint8_t>(UART_LSR_THR_EMPTY | UART_LSR_TRANSMITTER_EMPTY);
    return true;
}

void Uart16650Device::recv_byte(std::uint8_t c) {
    std::lock_guard<std::mutex> lock(queue_mutex_);
    if (queue_.size() >= UART_FIFO_CAPACITY) {
        std::fprintf(stderr, "uart receive queue is full, dropping byte\n");
        return;
    }

    queue_.push(c);
    lsr_ |= UART_LSR_DATA_READY;

    if (queue_.size() >= recv_fifo_trigger_byte_count_ && (ier_ & UART_IER_RX_AVAILABLE) != 0) {
        iir_ = 0b11000010;
        interrupt_ = true;
    }
}

void Uart16650Device::send_byte(std::uint8_t c) {
    std::lock_guard<std::mutex> lock(sender_mutex_);

    switch (mode_) {
        case Mode::Stream:
            if (stream_ == nullptr) {
                std::cout << c;
                std::cout.flush();
            } else {
                *stream_ << c;
                stream_->flush();
            }
            return;
        case Mode::Socket:
            if (send_socket_ < 0) {
                return;
            }
            if (send(send_socket_, &c, 1, 0) <= 0) {
                std::fprintf(stderr, "uart socket: send failed: %s\n", std::strerror(errno));
            }
            return;
        case Mode::None:
            std::cout << c;
            std::cout.flush();
            return;
    }
}

const char *Uart16650Device::type_name() const {
    return "uart16650";
}

Uart16650Device::~Uart16650Device() {
    if (recv_socket_ >= 0) {
        close(recv_socket_);
    }
    if (send_socket_ >= 0) {
        close(send_socket_);
    }
}
} // namespace kvm_host
