file(
    GLOB_RECURSE LIBFDT_SOURCES
    ${CMAKE_CURRENT_SOURCE_DIR}/*.c
)

add_library(libfdt OBJECT ${LIBFDT_SOURCES})
add_library(libfdt::libfdt ALIAS libfdt)

target_include_directories(libfdt PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})

target_compile_definitions(libfdt PRIVATE
    KERNEL_BUILD=1
)

set_target_properties(libfdt PROPERTIES
    ARCHIVE_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/lib
)
