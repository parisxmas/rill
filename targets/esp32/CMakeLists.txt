# An ESP-IDF application whose program is written in Rill. `build.sh` puts
# the compiled program and the runtime in main/lib before this is built.
cmake_minimum_required(VERSION 3.16)
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(rill_app)
