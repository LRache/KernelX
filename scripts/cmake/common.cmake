set(COMMON_FLAGS 
    -Wall 
    -Wextra
    -fno-common 
    -fno-builtin 
    -nostdlib 
    -ffreestanding
)

string(REPLACE ";" " " COMMON_FLAGS_STR "${COMMON_FLAGS}")

set(CMAKE_C_FLAGS   "${COMMON_FLAGS_STR} ${CMAKE_C_FLAGS}")
set(CMAKE_CXX_FLAGS "${COMMON_FLAGS_STR} ${CMAKE_CXX_FLAGS} -fno-exceptions -fno-rtti")
set(CMAKE_ASM_FLAGS "${COMMON_FLAGS_STR} ${CMAKE_ASM_FLAGS}")

if("${CONFIG_BACKTRACE}" STREQUAL "y")
    set(BACKTRACE_FLAG "-fno-omit-frame-pointer")
else()
    set(BACKTRACE_FLAG "")
endif()

set(CMAKE_C_FLAGS_DEBUG   "-ggdb -Og ${BACKTRACE_FLAG} ${CMAKE_C_FLAGS_DEBUG}")
set(CMAKE_CXX_FLAGS_DEBUG "-ggdb -Og ${BACKTRACE_FLAG} ${CMAKE_CXX_FLAGS_DEBUG}")
set(CMAKE_ASM_FLAGS_DEBUG "-ggdb -Og ${CMAKE_ASM_FLAGS_DEBUG}")
