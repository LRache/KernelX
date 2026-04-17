set(LWEXT4_ROOT ${CMAKE_CURRENT_SOURCE_DIR}/lwext4)
set(LWEXT4_GENERATED_DIR ${CMAKE_CURRENT_BINARY_DIR}/include/generated)

file(MAKE_DIRECTORY ${LWEXT4_GENERATED_DIR})
configure_file(
    ${CMAKE_CURRENT_SOURCE_DIR}/ext4_config.h.in
    ${LWEXT4_GENERATED_DIR}/ext4_config.h
    COPYONLY
)

file(
    GLOB LWEXT4_SOURCES
    ${LWEXT4_ROOT}/src/*.c
)

add_library(lwext4 OBJECT ${LWEXT4_SOURCES})

target_include_directories(lwext4 PUBLIC
    ${LWEXT4_ROOT}/include
    ${CMAKE_CURRENT_BINARY_DIR}/include
)

target_compile_definitions(lwext4 PRIVATE
    KERNEL_BUILD=1
)

set_target_properties(lwext4 PROPERTIES
    ARCHIVE_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/lib
)
