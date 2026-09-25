idf_component_register(SRCS "app.c")
# The Rill program, and the runtime it calls, as two archives.
add_prebuilt_library(rill_prog "${CMAKE_CURRENT_SOURCE_DIR}/lib/librill_prog.a")
add_prebuilt_library(rill_runtime "${CMAKE_CURRENT_SOURCE_DIR}/lib/librill_runtime.a")
target_link_libraries(${COMPONENT_LIB} PRIVATE rill_prog rill_runtime)
# The runtime reaches the C library by name, so it comes after the program.
target_link_libraries(rill_prog INTERFACE rill_runtime)
