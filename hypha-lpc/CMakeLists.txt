cmake_minimum_required(VERSION 3.20)
set(CMAKE_REQUIRED_LIBRARIES m)
project(hypha-lpc CXX)

add_library(
  hypha-lpc
  src/LPC.cpp
)

target_include_directories(
  hypha-lpc
  PUBLIC
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
  $<INSTALL_INTERFACE:include>
)

install(
  TARGETS hypha-lpc
  LIBRARY DESTINATION lib
  ARCHIVE DESTINATION lib
)

install(
  DIRECTORY include/
  DESTINATION include
)

include(FetchContent)

FetchContent_Declare(
  eigen
  GIT_REPOSITORY https://gitlab.com/libeigen/eigen.git
  GIT_TAG 3.4.0
)

FetchContent_MakeAvailable(eigen)

target_link_libraries(
  hypha-lpc
  Eigen3::Eigen
)
