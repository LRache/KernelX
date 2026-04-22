# LoongArch64 toolchain file for the vDSO build (CMake).
# Same rationale as clib/cmake/toolchain/loongarch64.cmake but emits a Linux-abi
# shared object (the vDSO the kernel maps into user space).
set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR loongarch64)

set(CMAKE_C_COMPILER   "clang" CACHE STRING "C Compiler")
set(CMAKE_CXX_COMPILER "clang++" CACHE STRING "C++ Compiler")
set(CMAKE_ASM_COMPILER "clang" CACHE STRING "ASM Compiler")
set(CMAKE_AR           "ar" CACHE STRING "Archiver")
set(CMAKE_LINKER       "clang -fuse-ld=lld" CACHE STRING "Linker")

set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)

set(ARCH_COMMON_FLAGS
    --target=loongarch64-linux-gnu
    -mcmodel=normal
    -march=loongarch64
    -mabi=lp64d
)

set(ARCH_COMMON_FLAGS_LIST ${ARCH_COMMON_FLAGS} -nostdlib)
string(REPLACE ";" " " ARCH_COMMON_FLAGS_STR "${ARCH_COMMON_FLAGS}")

set(CMAKE_C_FLAGS   "${ARCH_COMMON_FLAGS_STR} ${CMAKE_C_FLAGS}"  CACHE STRING "C Flags")
set(CMAKE_CXX_FLAGS "${ARCH_COMMON_FLAGS_STR} ${CMAKE_CXX_FLAGS}" CACHE STRING "CXX Flags")
set(CMAKE_ASM_FLAGS "${ARCH_COMMON_FLAGS_STR} ${CMAKE_ASM_FLAGS}" CACHE STRING "ASM Flags")
set(CMAKE_SHARED_LINKER_FLAGS "${ARCH_COMMON_FLAGS_STR} -fuse-ld=lld -nostdlib ${CMAKE_SHARED_LINKER_FLAGS}" CACHE STRING "Shared Linker Flags")
