add_library(lwext4 OBJECT)

target_include_directories(lwext4 PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/include)

file(GLOB_RECURSE SRC
    ${CMAKE_CURRENT_SOURCE_DIR}/src/*.c
)
target_sources(lwext4 PRIVATE ${SRC})
