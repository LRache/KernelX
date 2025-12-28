cmake_minimum_required(VERSION 3.16)

include(${CMAKE_SOURCE_DIR}/cmake/filelist.cmake)

project(kernelx_clib C CXX ASM)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

add_subdirectory(${CMAKE_SOURCE_DIR}/lib/tlsf)
add_subdirectory(${CMAKE_SOURCE_DIR}/lib/libfdt)

add_library(kernelx_clib STATIC ${SRCS})
target_include_directories(kernelx_clib PUBLIC ${CMAKE_SOURCE_DIR}/include)
target_link_libraries(kernelx_clib PRIVATE tlsf libfdt)

include(${KERNELX_HOME}/scripts/cmake/env.cmake)

set(COMMON_FLAGS -Wall -Wextra -fno-common -fno-builtin -nostdlib -ffreestanding)
list(APPEND COMMON_FLAGS -g -ggdb -Og)
list(APPEND COMMON_FLAGS ${ARCH_COMMON_FLAGS_LIST})

message("COMMON_FLAGS: ${COMMON_FLAGS}")

target_compile_options(tlsf PRIVATE ${COMMON_FLAGS})
target_compile_options(libfdt PRIVATE ${COMMON_FLAGS})
target_compile_options(kernelx_clib PRIVATE ${COMMON_FLAGS})
