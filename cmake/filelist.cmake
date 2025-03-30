file(
    GLOB_RECURSE SRCS
    ${CMAKE_SOURCE_DIR}/src/arch/${ARCH}/*.*
    ${CMAKE_SOURCE_DIR}/src/mem/**.cpp
    ${CMAKE_SOURCE_DIR}/src/*.cpp
)

list(APPEND SRCS ${CMAKE_SOURCE_DIR}/lib/tinyprintf/tinyprintf.c)
list(APPEND SRCS ${CMAKE_SOURCE_DIR}/lib/tlsf/tlsf.c)
