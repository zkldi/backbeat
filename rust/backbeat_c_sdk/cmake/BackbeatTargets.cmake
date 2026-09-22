if(TARGET Backbeat::Backbeat)
    set(Backbeat_FOUND TRUE)
    return()
endif()

include(CMakeFindDependencyMacro)

if(WIN32)
    set(_backbeat_archive backbeat_c_sdk.lib)
    set(_backbeat_sqlite_archive sqlite3.lib)
    set(_backbeat_system_libraries
        advapi32 bcrypt kernel32 ntdll ole32 shell32 userenv ws2_32)
else()
    set(_backbeat_archive libbackbeat_c_sdk.a)
    set(_backbeat_sqlite_archive libsqlite3.a)
    find_dependency(Threads)
    if(APPLE)
        set(_backbeat_system_libraries
            "-framework Security"
            "-framework SystemConfiguration"
            "-framework CoreFoundation"
            iconv Threads::Threads ${CMAKE_DL_LIBS})
    else()
        find_dependency(OpenSSL COMPONENTS SSL Crypto)
        set(_backbeat_system_libraries
            OpenSSL::SSL OpenSSL::Crypto gcc_s util rt Threads::Threads m ${CMAKE_DL_LIBS})
    endif()
endif()

set(_backbeat_include_dir "${_backbeat_prefix}/include")
set(_backbeat_library "${_backbeat_prefix}/lib/${_backbeat_archive}")
set(_backbeat_sqlite_library "${_backbeat_prefix}/lib/${_backbeat_sqlite_archive}")

if(NOT EXISTS "${_backbeat_library}")
    set(Backbeat_FOUND FALSE)
    set(Backbeat_NOT_FOUND_MESSAGE "Backbeat archive not found: ${_backbeat_library}")
    return()
endif()

if(NOT EXISTS "${_backbeat_include_dir}/backbeat.h")
    set(Backbeat_FOUND FALSE)
    set(Backbeat_NOT_FOUND_MESSAGE "Backbeat header not found: ${_backbeat_include_dir}/backbeat.h")
    return()
endif()

# A consumer or package manager may supply this target to select its own
# compatible SQLite build. Otherwise prefer an existing SQLite target, the
# archive shipped in the SDK, then a separately installed SQLite package.
set(_backbeat_archives Backbeat)
if(NOT TARGET Backbeat::SQLite)
    if(NOT TARGET SQLite::SQLite3 AND NOT EXISTS "${_backbeat_sqlite_library}")
        find_dependency(SQLite3 3.38)
    endif()
    if(TARGET SQLite::SQLite3)
        add_library(Backbeat::SQLite INTERFACE IMPORTED)
        set_target_properties(Backbeat::SQLite PROPERTIES
            INTERFACE_LINK_LIBRARIES SQLite::SQLite3)
    else()
        list(APPEND _backbeat_archives SQLite)
    endif()
endif()

foreach(_backbeat_component IN LISTS _backbeat_archives)
    if(_backbeat_component STREQUAL "Backbeat")
        set(_backbeat_filename "${_backbeat_archive}")
    else()
        set(_backbeat_filename "${_backbeat_sqlite_archive}")
    endif()
    add_library(Backbeat::${_backbeat_component} STATIC IMPORTED)
    set_target_properties(Backbeat::${_backbeat_component} PROPERTIES
        IMPORTED_CONFIGURATIONS RELEASE
        IMPORTED_LOCATION "${_backbeat_prefix}/lib/${_backbeat_filename}"
        IMPORTED_LOCATION_RELEASE "${_backbeat_prefix}/lib/${_backbeat_filename}"
        MAP_IMPORTED_CONFIG_RELWITHDEBINFO Release
        MAP_IMPORTED_CONFIG_MINSIZEREL Release
        INTERFACE_INCLUDE_DIRECTORIES "${_backbeat_include_dir}")
    if(EXISTS "${_backbeat_prefix}/debug/lib/${_backbeat_filename}")
        set_property(TARGET Backbeat::${_backbeat_component} APPEND PROPERTY
            IMPORTED_CONFIGURATIONS DEBUG)
        set_target_properties(Backbeat::${_backbeat_component} PROPERTIES
            IMPORTED_LOCATION_DEBUG "${_backbeat_prefix}/debug/lib/${_backbeat_filename}")
    else()
        set_target_properties(Backbeat::${_backbeat_component} PROPERTIES
            MAP_IMPORTED_CONFIG_DEBUG Release)
    endif()
endforeach()

set_target_properties(Backbeat::Backbeat PROPERTIES
    INTERFACE_LINK_LIBRARIES "Backbeat::SQLite;${_backbeat_extra_libraries};${_backbeat_system_libraries}")
if(NOT WIN32 AND "SQLite" IN_LIST _backbeat_archives)
    set_target_properties(Backbeat::SQLite PROPERTIES
        INTERFACE_LINK_LIBRARIES "Threads::Threads;${CMAKE_DL_LIBS};m")
endif()

set(Backbeat_FOUND TRUE)

unset(_backbeat_include_dir)
unset(_backbeat_library)
unset(_backbeat_prefix)
unset(_backbeat_sqlite_library)
unset(_backbeat_system_libraries)
unset(_backbeat_archive)
unset(_backbeat_sqlite_archive)
unset(_backbeat_archives)
unset(_backbeat_component)
unset(_backbeat_filename)
unset(_backbeat_extra_libraries)
