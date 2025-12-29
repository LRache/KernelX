cmake_minimum_required(VERSION 3.16)

include(${CMAKE_SOURCE_DIR}/cmake/filelist.cmake)
include(${KERNELX_HOME}/scripts/cmake/common.cmake)

project(kernelx_clib C CXX ASM)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

add_subdirectory(${CMAKE_SOURCE_DIR}/lib/tlsf)
add_subdirectory(${CMAKE_SOURCE_DIR}/lib/libfdt)

add_library(kernelx_clib STATIC ${SRCS})
target_include_directories(kernelx_clib PUBLIC ${CMAKE_SOURCE_DIR}/include)
target_link_libraries(kernelx_clib PRIVATE tlsf libfdt)

# include(${KERNELX_HOME}/scripts/cmake/env.cmake)

# target_compile_options(tlsf PRIVATE ${COMMON_FLAGS})
# target_compile_options(libfdt PRIVATE ${COMMON_FLAGS})
# target_compile_options(kernelx_clib PRIVATE ${COMMON_FLAGS})
