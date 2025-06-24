cmake_minimum_required(VERSION 3.20)
project(hypha-lpc-sys NONE)
include(FetchContent)

FetchContent_Declare(
  hypha-lpc
  SOURCE_DIR "${CMAKE_SOURCE_DIR}/../hypha-lpc"
)

FetchContent_MakeAvailable(hypha-lpc)
