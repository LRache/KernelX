cmake_minimum_required(VERSION 3.16)

project(kernelx_vdso C ASM)
enable_language(ASM C)

if(NOT DEFINED ARCH)
    message(FATAL_ERROR "ARCH is not defined")
endif()

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

set(CMAKE_ASM_COMPILER clang)
set(CMAKE_C_COMPILER clang)

# Use clang as linker to handle RISC-V targets properly
set(CMAKE_LINKER clang)

# Fix for CMAKE_ASM_CREATE_SHARED_LIBRARY not being set
if(NOT CMAKE_ASM_CREATE_SHARED_LIBRARY)
    set(CMAKE_ASM_CREATE_SHARED_LIBRARY
        "<CMAKE_C_COMPILER> <CMAKE_SHARED_LIBRARY_ASM_FLAGS> <LANGUAGE_COMPILE_FLAGS> <LINK_FLAGS> <CMAKE_SHARED_LIBRARY_CREATE_ASM_FLAGS> <SONAME_FLAG><TARGET_SONAME> -o <TARGET> <OBJECTS> <LINK_LIBRARIES>"
    )
endif()

include(${KERNELX_HOME}/scripts/cmake/env.cmake)

set(COMMON_FLAGS -Wall -Wextra -fno-common -fno-builtin -nostdlib -ffreestanding -fPIC)
list(APPEND COMMON_FLAGS -g -ggdb -Og)
list(APPEND COMMON_FLAGS ${ARCH_COMMON_FLAGS_LIST})

file(
    GLOB_RECURSE SRCS
    ${CMAKE_SOURCE_DIR}/src/arch/${ARCH}/*.c
    ${CMAKE_SOURCE_DIR}/src/arch/${ARCH}/*.S
)

add_library(vdso SHARED ${SRCS})
target_compile_options(vdso PRIVATE ${COMMON_FLAGS})
target_link_options(vdso PRIVATE ${ARCH_COMMON_FLAGS_LIST})
set_target_properties(vdso PROPERTIES 
    POSITION_INDEPENDENT_CODE ON
    LINKER_LANGUAGE C
)
