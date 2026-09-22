cmake_minimum_required(VERSION 3.14)
project(backbeat_c_examples LANGUAGES C CXX)

find_package(Backbeat CONFIG REQUIRED)

add_executable(store_info store_info.c)
add_executable(asset_read asset_read.c)
add_executable(version version.c)
add_executable(cpp_link cpp_link.cpp)

set_target_properties(store_info asset_read version PROPERTIES C_STANDARD 11)
set_target_properties(cpp_link PROPERTIES CXX_STANDARD 17)

foreach(target store_info asset_read version cpp_link)
    target_link_libraries(${target} PRIVATE Backbeat::Backbeat)
endforeach()

enable_testing()
add_test(NAME version COMMAND version)
add_test(NAME cpp_link COMMAND cpp_link)
