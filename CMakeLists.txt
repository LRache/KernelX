cmake_minimum_required(VERSION 3.5)
project(KernelX)
enable_language(CXX C ASM)

set(KERNELX_TARGET "kernelx-${ARCH}")

include(${CMAKE_SOURCE_DIR}/cmake/filelist.cmake)
add_executable(
    ${KERNELX_TARGET} 
    ${SRCS}
)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(COMMON_C_FLAGS "-Wall -Wextra -O3 -Og -ffreestanding -nostdlib -lgcc -fno-stack-protector -fno-builtin -fno-strict-aliasing -fno-omit-frame-pointer -fno-common")
set(CMAKE_C_FLAGS   "${COMMON_C_FLAGS} ${CMAKE_C_FLAGS}   ")
set(CMAKE_CXX_FLAGS "${COMMON_C_FLAGS} ${CMAKE_CXX_FLAGS} -fno-rtti -fno-exceptions -fno-use-cxa-atexit")

target_link_options(${KERNELX_TARGET} PRIVATE
    -nostdlib
    -T${CMAKE_SOURCE_DIR}/scripts/linker.ld
)

set(CMAKE_C_LINK_EXECUTABLE
    "${CMAKE_LINKER} <CMAKE_C_LINK_FLAGS> <LINK_FLAGS> <OBJECTS> -o <TARGET> <LINK_LIBRARIES>"
)

set(CMAKE_CXX_LINK_EXECUTABLE
    "${CMAKE_LINKER} <CMAKE_CXX_LINK_FLAGS> <LINK_FLAGS> <OBJECTS> -o <TARGET> <LINK_LIBRARIES>"
)

target_include_directories(
    ${KERNELX_TARGET}
    PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/lib/tlsf
    ${CMAKE_SOURCE_DIR}/lib/tinyprintf
)

# Lib ETL
add_subdirectory(lib/etl)
target_link_libraries(${KERNELX_TARGET} PRIVATE etl::etl)
