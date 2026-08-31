cmake_minimum_required(VERSION 3.16)

include(${CMAKE_SOURCE_DIR}/cmake/filelist.cmake)
include(${KERNELX_HOME}/scripts/cmake/common.cmake)

project(kernelx_clib C CXX ASM)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

add_subdirectory(${CMAKE_SOURCE_DIR}/lib/tlsf)
add_subdirectory(${CMAKE_SOURCE_DIR}/lib/libfdt)
target_include_directories(libfdt PRIVATE ${CMAKE_SOURCE_DIR}/include)

if("${CONFIG_ENABLE_LWEXT4}" STREQUAL "y")
    add_subdirectory(${CMAKE_SOURCE_DIR}/lib/lwext4)
    target_include_directories(lwext4 PRIVATE ${CMAKE_SOURCE_DIR}/include)
else()
    list(FILTER SRCS EXCLUDE REGEX "/src/fs/ext4/")
endif()

add_library(kernelx_clib STATIC ${SRCS})
target_include_directories(kernelx_clib PUBLIC ${CMAKE_SOURCE_DIR}/include)
target_link_libraries(kernelx_clib PRIVATE tlsf libfdt)

if("${CONFIG_ENABLE_LWEXT4}" STREQUAL "y")
    target_link_libraries(kernelx_clib PRIVATE lwext4)
endif()
