cmake_minimum_required(VERSION 3.20)

project(kvm_host LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

set(CMAKE_RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/output")

add_executable(minimal_vm minimal_vm.cpp)

target_compile_options(minimal_vm PRIVATE
    -Wall
    -Wextra
    -fno-exceptions
    -fno-rtti
)
