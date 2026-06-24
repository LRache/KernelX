set(COMMON_FLAGS 
    -Wall 
    -Wextra
    -fno-common 
    -fno-builtin 
    -nostdlib 
    -ffreestanding
)

string(REPLACE ";" " " COMMON_FLAGS_STR "${COMMON_FLAGS}")

set(FLAGS_WITH_DEBUG_INFO
    CMAKE_C_FLAGS
    CMAKE_CXX_FLAGS
    CMAKE_ASM_FLAGS
    CMAKE_C_FLAGS_DEBUG
    CMAKE_CXX_FLAGS_DEBUG
    CMAKE_ASM_FLAGS_DEBUG
)

foreach(flag_var ${FLAGS_WITH_DEBUG_INFO})
    string(REPLACE "-gdwarf-4" "" ${flag_var} "${${flag_var}}")
    string(REPLACE "-ggdb" "" ${flag_var} "${${flag_var}}")
endforeach()

foreach(flag_var CMAKE_C_FLAGS CMAKE_CXX_FLAGS CMAKE_C_FLAGS_DEBUG CMAKE_CXX_FLAGS_DEBUG)
    string(REPLACE "-fno-omit-frame-pointer" "" ${flag_var} "${${flag_var}}")
endforeach()

if(NOT "${CONFIG_DWARF}" STREQUAL "y")
    foreach(flag_var ${FLAGS_WITH_DEBUG_INFO})
        string(REPLACE "-fno-limit-debug-info" "" ${flag_var} "${${flag_var}}")
    endforeach()
endif()

if("${CONFIG_DWARF}" STREQUAL "y")
    set(DWARF_FLAG "-gdwarf-4")
else()
    set(DWARF_FLAG "")
endif()

if("${CONFIG_BACKTRACE}" STREQUAL "y")
    set(BACKTRACE_FLAG "-fno-omit-frame-pointer")
else()
    set(BACKTRACE_FLAG "")
endif()

set(CMAKE_C_FLAGS   "${COMMON_FLAGS_STR} ${DWARF_FLAG} ${BACKTRACE_FLAG} ${CMAKE_C_FLAGS}")
set(CMAKE_CXX_FLAGS "${COMMON_FLAGS_STR} ${DWARF_FLAG} ${BACKTRACE_FLAG} ${CMAKE_CXX_FLAGS} -fno-exceptions -fno-rtti")
set(CMAKE_ASM_FLAGS "${COMMON_FLAGS_STR} ${DWARF_FLAG} ${CMAKE_ASM_FLAGS}")

set(CMAKE_C_FLAGS_DEBUG   "-Og ${CMAKE_C_FLAGS_DEBUG}")
set(CMAKE_CXX_FLAGS_DEBUG "-Og ${CMAKE_CXX_FLAGS_DEBUG}")
set(CMAKE_ASM_FLAGS_DEBUG "-Og ${CMAKE_ASM_FLAGS_DEBUG}")
