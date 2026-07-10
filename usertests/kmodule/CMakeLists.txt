file(GLOB KMODULE_TEST_DIRS CONFIGURE_DEPENDS RELATIVE "${CMAKE_CURRENT_LIST_DIR}" "*")

foreach(test_dir IN LISTS KMODULE_TEST_DIRS)
    if(IS_DIRECTORY "${CMAKE_CURRENT_LIST_DIR}/${test_dir}"
        AND EXISTS "${CMAKE_CURRENT_LIST_DIR}/${test_dir}/CMakeLists.txt")
        add_subdirectory("${test_dir}")
    endif()
endforeach()
