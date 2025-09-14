cmake_minimum_required(VERSION 3.16)

include(${CMAKE_SOURCE_DIR}/cmake/filelist.cmake)

set(CMAKE_ASM_COMPILER clang)
set(CMAKE_C_COMPILER clang)
set(CMAKE_CXX_COMPILER clang++)

project(kernelx_clib C ASM)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

add_subdirectory(${CMAKE_SOURCE_DIR}/lib/tlsf)
add_subdirectory(${CMAKE_SOURCE_DIR}/lib/lwext4)

add_library(kernelx_clib STATIC ${SRCS})
target_include_directories(kernelx_clib PUBLIC ${CMAKE_SOURCE_DIR}/include)
target_include_directories(kernelx_clib PRIVATE ${CMAKE_SOURCE_DIR}/lib/lwext4/include)
target_link_libraries(kernelx_clib PRIVATE tlsf)
target_link_libraries(kernelx_clib PRIVATE lwext4)

set(COMMON_FLAGS -Wall -Wextra -fno-common -fno-builtin -nostdlib -ffreestanding)
set(COMMON_FLAGS -g -ggdb -Og)
set(COMMON_FLAGS ${COMMON_FLAGS} --target=${TARGET})
set(COMMON_FLAGS ${COMMON_FLAGS} -mabi=lp64d -march=rv64gc)

target_compile_options(tlsf PRIVATE ${COMMON_FLAGS})
target_compile_options(lwext4 PRIVATE ${COMMON_FLAGS})
target_compile_options(kernelx_clib PRIVATE ${COMMON_FLAGS})
