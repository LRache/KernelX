include(${KERNELX_HOME}/scripts/cmake/parse_config.cmake)

if(ARCH STREQUAL "riscv")
    if (ARCH_BITS STREQUAL "64")
        parse_config(${KERNELX_HOME}/scripts/flags/riscv64.env)
        message("ARCH_COMMON_FLAGS=${ARCH_COMMON_FLAGS}")
    else()
        message(FATAL_ERROR "Unsupported riscv architecture bits: ${ARCH_BITS}")
    endif()
else()
    message(FATAL_ERROR "Unsupported architecture: ${ARCH}")
endif()

string(REPLACE " " ";" ARCH_COMMON_FLAGS_LIST ${ARCH_COMMON_FLAGS})
