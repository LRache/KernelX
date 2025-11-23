if(NOT DEFINED ENV{ARCH})
    set(ARCH "x86_64")
else()
    set(ARCH $ENV{ARCH})
endif()

# Name of the target
set(CMAKE_SYSTEM_NAME "Linux")
set(CMAKE_SYSTEM_PROCESSOR ${ARCH})

# Toolchain settings
set(TOOLCHAIN_PREFIX ${ARCH}-linux-gnu)

set(CMAKE_C_COMPILER    ${TOOLCHAIN_PREFIX}-gcc)
set(CMAKE_CXX_COMPILER  ${TOOLCHAIN_PREFIX}-c++)
set(AS                  ${TOOLCHAIN_PREFIX}-as)
set(AR                  ${TOOLCHAIN_PREFIX}-ar)
set(OBJCOPY             ${TOOLCHAIN_PREFIX}-objcopy)
set(OBJDUMP             ${TOOLCHAIN_PREFIX}-objdump)
set(SIZE                ${TOOLCHAIN_PREFIX}-size)

set(LD_FLAGS "-nolibc -nostdlib -static --gc-sections -nostartfiles")

set(CMAKE_C_FLAGS   "-std=gnu99 -fdata-sections -ffunction-sections -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0" CACHE INTERNAL "c compiler flags")
set(CMAKE_CXX_FLAGS "-fdata-sections -ffunction-sections -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0" CACHE INTERNAL "cxx compiler flags")
set(CMAKE_ASM_FLAGS "" CACHE INTERNAL "asm compiler flags")

if(ARCH STREQUAL "x86_64")
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -mno-sse")
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -mno-sse")
elseif (ARCH STREQUAL "aarch64")
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -mgeneral-regs-only")
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -mgeneral-regs-only")
elseif (ARCH STREQUAL "riscv64")
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -march=rv64gc -mabi=lp64d -mcmodel=medany")
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -march=rv64gc -mabi=lp64d -mcmodel=medany")
elseif (ARCH STREQUAL "loongarch64")
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -msoft-float")
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -msoft-float")
endif()

set(CMAKE_C_FLAGS "-fPIC -fno-builtin -ffreestanding -fno-omit-frame-pointer ${CMAKE_C_FLAGS}")
set(CMAKE_CXX_FLAGS "-fPIC -nostdinc -fno-builtin -ffreestanding -fno-omit-frame-pointer ${CMAKE_CXX_FLAGS}")

if (APPLE)
    set(CMAKE_EXE_LINKER_FLAGS "-dead_strip" CACHE INTERNAL "exe link flags")
else (APPLE)
    set(CMAKE_EXE_LINKER_FLAGS "-Wl,--gc-sections" CACHE INTERNAL "exe link flags")
endif (APPLE)

SET(CMAKE_C_FLAGS_DEBUG "-O0 -g -ggdb3" CACHE INTERNAL "c debug compiler flags")
SET(CMAKE_CXX_FLAGS_DEBUG "-O0 -g -ggdb3" CACHE INTERNAL "cxx debug compiler flags")
SET(CMAKE_ASM_FLAGS_DEBUG "-g -ggdb3" CACHE INTERNAL "asm debug compiler flags")

SET(CMAKE_C_FLAGS_RELEASE "-O2 -g -ggdb3" CACHE INTERNAL "c release compiler flags")
SET(CMAKE_CXX_FLAGS_RELEASE "-O2 -g -ggdb3" CACHE INTERNAL "cxx release compiler flags")
SET(CMAKE_ASM_FLAGS_RELEASE "" CACHE INTERNAL "asm release compiler flags")
