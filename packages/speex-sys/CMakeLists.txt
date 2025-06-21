# This CMake script fetches and builds the SpeexDSP source code.
# It is based on the vcpkg port: https://github.com/microsoft/vcpkg/blob/master/ports/speexdsp/CMakeLists.txt

cmake_minimum_required(VERSION 4.0)
project(NONE)
include(FetchContent)

FetchContent_Populate(
  speexdsp
  GIT_REPOSITORY https://gitlab.xiph.org/xiph/speexdsp.git
  GIT_TAG SpeexDSP-1.2.1
)

set(USE_SSE OFF)
set(USE_NEON OFF)
set(FIXED_POINT OFF)

if(CMAKE_SYSTEM_PROCESSOR MATCHES "x86_64|AMD64|i686|x86")
    set(USE_SSE ON)
elseif(CMAKE_SYSTEM_PROCESSOR MATCHES "aarch64|arm")
    set(USE_NEON ON)
endif()

set(LIBSPEEXDSP_VERSION "1.2.0")
set(LIBSPEEXDSP_SOVERSION "1")
project(libspeexdsp)

set(LIBSPEEXDSP_SOURCES
    "${speexdsp_SOURCE_DIR}/libspeexdsp/buffer.c"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/fftwrap.c"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/filterbank.c"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/jitter.c"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/mdf.c"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/preprocess.c"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/resample.c"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/scal.c"
)
set(LIBSPEEXDSP_HEADERS
    "${speexdsp_SOURCE_DIR}/libspeexdsp/arch.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/bfin.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/fftwrap.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/filterbank.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/fixed_arm4.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/fixed_arm5e.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/fixed_bfin.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/fixed_debug.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/fixed_generic.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/math_approx.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/misc_bfin.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/os_support.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/pseudofloat.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/resample_neon.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/resample_sse.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/smallft.h"
    "${speexdsp_SOURCE_DIR}/libspeexdsp/vorbis_psy.h"
    "${speexdsp_SOURCE_DIR}/include/speex/speex_buffer.h"
)
set(LIBSPEEXDSP_HEADERS_PUBLIC
    "${speexdsp_SOURCE_DIR}/include/speex/speex_echo.h"
    "${speexdsp_SOURCE_DIR}/include/speex/speex_jitter.h"
    "${speexdsp_SOURCE_DIR}/include/speex/speex_preprocess.h"
    "${speexdsp_SOURCE_DIR}/include/speex/speex_resampler.h"
    "${speexdsp_SOURCE_DIR}/include/speex/speexdsp_types.h"
)

set(CMAKE_C_VISIBILITY_PRESET hidden)
if (NOT BUILD_SHARED_LIBS)
    add_definitions("-DEXPORT=")
elseif(WIN32)
    add_definitions("-DEXPORT=")
    list(APPEND LIBSPEEXDSP_SOURCES "${speexdsp_SOURCE_DIR}/win32/libspeexdsp.def")
else()
    add_definitions("-DEXPORT=__attribute__((visibility(\"default\")))")
endif()

if (USE_SSE AND NOT FIXED_POINT)
    add_definitions(-DUSE_SSE -DUSE_SSE2)
endif()
if (USE_NEON AND NOT MSVC)
    add_definitions(-DUSE_NEON)
endif()

if (FIXED_POINT)
    add_definitions(-DFIXED_POINT -DUSE_KISS_FFT)
    list(APPEND LIBSPEEXDSP_SOURCES
        "${speexdsp_SOURCE_DIR}/libspeexdsp/kiss_fft.c"
        "${speexdsp_SOURCE_DIR}/libspeexdsp/kiss_fftr.c"
    )
    list(APPEND LIBSPEEXDSP_HEADERS
        "${speexdsp_SOURCE_DIR}/libspeexdsp/_kiss_fft_guts.h"
        "${speexdsp_SOURCE_DIR}/libspeexdsp/kiss_fft.h"
        "${speexdsp_SOURCE_DIR}/libspeexdsp/kiss_fftr.h"
    )
else()
    add_definitions(-DFLOATING_POINT -DUSE_SMALLFT)
    list(APPEND LIBSPEEXDSP_SOURCES "${speexdsp_SOURCE_DIR}/libspeexdsp/smallft.c")
endif()

if (NOT MSVC)
    add_definitions(-DVAR_ARRAYS)
endif()

# Basic speexdsp_config_types.h for Linux support
set(INCLUDE_STDINT "#include <stdint.h>")
set(SIZE16 "int16_t")
set(USIZE16 "uint16_t")
set(SIZE32 "int32_t")
set(USIZE32 "uint32_t")
configure_file("${speexdsp_SOURCE_DIR}/include/speex/speexdsp_config_types.h.in"
               "${CMAKE_CURRENT_BINARY_DIR}/speexdsp_config_types.h" @ONLY)
list(APPEND LIBSPEEXDSP_HEADERS_PUBLIC "${CMAKE_CURRENT_BINARY_DIR}/speexdsp_config_types.h")

include_directories("${speexdsp_SOURCE_DIR}/include")
include_directories("${CMAKE_CURRENT_BINARY_DIR}")

add_library(speexdsp ${LIBSPEEXDSP_SOURCES} ${LIBSPEEXDSP_HEADERS})
set_target_properties(speexdsp PROPERTIES PUBLIC_HEADER "${LIBSPEEXDSP_HEADERS_PUBLIC}")
set_target_properties(speexdsp PROPERTIES VERSION "${LIBSPEEXDSP_VERSION}")
set_target_properties(speexdsp PROPERTIES SOVERSION "${LIBSPEEXDSP_SOVERSION}")
if (WIN32)
    set_target_properties(speexdsp PROPERTIES RUNTIME_OUTPUT_NAME "libspeexdsp")
endif()

install(TARGETS speexdsp
        ARCHIVE DESTINATION "lib"
        RUNTIME DESTINATION "bin"
        PUBLIC_HEADER DESTINATION "include/speex")
