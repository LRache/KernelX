# LoongArch64 toolchain file for clib (CMake).
# Uses Homebrew clang + lld; no GNU cross-binutils required on macOS/Linux host.
set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_SYSTEM_PROCESSOR loongarch64)

set(CMAKE_C_COMPILER   "clang" CACHE STRING "C Compiler")
set(CMAKE_CXX_COMPILER "clang++" CACHE STRING "C++ Compiler")
set(CMAKE_ASM_COMPILER "clang" CACHE STRING "ASM Compiler")
set(CMAKE_AR           "ar" CACHE STRING "Archiver")
set(CMAKE_LINKER       "clang -fuse-ld=lld" CACHE STRING "Linker")

set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)

# NB: bare-metal -elf triple; LP64D ABI (hard float), matches loongarch64-unknown-none.
set(ARCH_COMMON_FLAGS
    --target=loongarch64-unknown-elf
    -mcmodel=normal
    -march=loongarch64
    -mabi=lp64d
    -fPIE
    -gdwarf-4
    -fno-limit-debug-info
)

if(SYSROOT)
    set(CMAKE_SYSROOT ${SYSROOT})
    list(APPEND ARCH_COMMON_FLAGS
        --sysroot=${SYSROOT}
        -isystem ${SYSROOT}/include
        -isystem ${SYSROOT}/usr/include
    )
endif()

set(ARCH_COMMON_FLAGS_LIST ${ARCH_COMMON_FLAGS} -nostdlib)
string(REPLACE ";" " " ARCH_COMMON_FLAGS_STR "${ARCH_COMMON_FLAGS}")

set(CMAKE_C_FLAGS   "${ARCH_COMMON_FLAGS_STR} ${CMAKE_C_FLAGS}"  CACHE STRING "C Flags")
set(CMAKE_CXX_FLAGS "${ARCH_COMMON_FLAGS_STR} ${CMAKE_CXX_FLAGS}" CACHE STRING "CXX Flags")
set(CMAKE_ASM_FLAGS "${ARCH_COMMON_FLAGS_STR} ${CMAKE_ASM_FLAGS}" CACHE STRING "ASM Flags")
set(CMAKE_SHARED_LINKER_FLAGS "${ARCH_COMMON_FLAGS_STR} -nostdlib ${CMAKE_SHARED_LINKER_FLAGS}" CACHE STRING "Shared Linker Flags")
