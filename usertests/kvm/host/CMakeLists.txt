cmake_minimum_required(VERSION 3.20)

project(kxemu-kvm LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

set(CMAKE_RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/output")

add_executable(kxemu-kvm
    src/main.cpp
    src/guest_boot.cpp
    src/linux_dtb.cpp
    src/device/bus.cpp
    src/device/mmio.cpp
    src/device/uart.cpp
    src/device/plic.cpp
    src/kvm.cpp
    src/vcpu/vcpu.cpp
    src/vcpu/abi.cpp
    src/vcpu/mmio.cpp
    src/vcpu/opensbi.cpp
    src/vcpu/pagetable.cpp
)

target_include_directories(kxemu-kvm PRIVATE include)

target_compile_options(kxemu-kvm PRIVATE
    -Wall
    -Wextra
    -Werror
    -fno-exceptions
    -fno-rtti
)
