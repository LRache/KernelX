set(TLSF_SOURCES
   ${CMAKE_CURRENT_SOURCE_DIR}/tlsf.c
)

add_library(tlsf OBJECT ${TLSF_SOURCES})
add_library(tlsf::tlsf ALIAS tlsf)

target_include_directories(tlsf PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})

target_compile_definitions(tlsf PRIVATE
    KERNEL_BUILD=1
)

set_target_properties(tlsf PROPERTIES
    ARCHIVE_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/lib
)
