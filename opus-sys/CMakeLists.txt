cmake_minimum_required(VERSION 3.20)
project(NONE)
include(FetchContent)

FetchContent_Declare(
  opus
  GIT_REPOSITORY https://github.com/xiph/opus.git
  GIT_TAG v1.5.2
)

FetchContent_MakeAvailable(opus)
