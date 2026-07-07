cmake_minimum_required(VERSION 3.16)

set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
project(KernelXKModule C)

if(KMODULE_ARCH STREQUAL "riscv")
    set(KMODULE_ARCH_COMPILE_OPTIONS
        -mcmodel=medany
        -march=rv64gc
        -mabi=lp64d
        -mno-relax
    )
elseif(KMODULE_ARCH STREQUAL "loongarch")
    set(KMODULE_ARCH_COMPILE_OPTIONS
        -march=loongarch64
        -mabi=lp64d
        -mcmodel=medium
        -fno-plt
        -mno-relax
        -mdirect-extern-access
    )
else()
    message(FATAL_ERROR "unsupported kmodule architecture: ${KMODULE_ARCH}")
endif()

set(KMODULE_SDK_DIR "${CMAKE_CURRENT_LIST_DIR}")
set(KMODULE_IMPLS_DIR "${CMAKE_CURRENT_LIST_DIR}/../kmodule-impls" CACHE PATH "Kernel module implementations")
set(KMODULE_OUTPUT_DIR "${CMAKE_BINARY_DIR}" CACHE PATH "Kernel module output directory")

set(KMODULE_COMPILE_OPTIONS
    -ffreestanding
    -fno-builtin
    -nostdlib
    -fno-pic
    -fno-pie
    -fno-unwind-tables
    -fno-asynchronous-unwind-tables
    ${KMODULE_ARCH_COMPILE_OPTIONS}
)

function(kmodule_configure_object target)
    target_include_directories(${target} PRIVATE "${KMODULE_SDK_DIR}/include")
    target_compile_options(${target} PRIVATE ${KMODULE_COMPILE_OPTIONS})
endfunction()

function(kmodule_add_module name)
    cmake_parse_arguments(MODULE "" "" "SOURCES" ${ARGN})
    if(NOT MODULE_SOURCES)
        message(FATAL_ERROR "kmodule_add_module(${name}) requires SOURCES")
    endif()

    set(object_target "${name}_objects")
    set(output_file "${KMODULE_OUTPUT_DIR}/${name}.ko")

    add_library(${object_target} OBJECT ${MODULE_SOURCES})
    kmodule_configure_object(${object_target})

    add_custom_command(
        OUTPUT "${output_file}"
        COMMAND ${CMAKE_COMMAND} -E make_directory "${KMODULE_OUTPUT_DIR}"
        COMMAND ${CMAKE_C_COMPILER} -nostdlib -r $<TARGET_OBJECTS:${object_target}> -o "${output_file}"
        DEPENDS ${object_target}
        VERBATIM
    )
    add_custom_target(${name} ALL DEPENDS "${output_file}")
endfunction()

add_subdirectory("${KMODULE_IMPLS_DIR}" "${CMAKE_BINARY_DIR}/impls")
