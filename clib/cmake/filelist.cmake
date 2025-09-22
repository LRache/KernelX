file(
    GLOB_RECURSE SRCS
    ${CMAKE_SOURCE_DIR}/src/klib/*.*
    ${CMAKE_SOURCE_DIR}/src/fs/*.*
    ${CMAKE_SOURCE_DIR}/src/arch/${ARCH}/*.*
    ${CMAKE_SOURCE_DIR}/src/platform/${PLATFORM}/*.*
)
