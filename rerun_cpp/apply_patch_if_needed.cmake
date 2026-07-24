# Portable ExternalProject patch helper. The previous command used Unix shell
# operators (`&&`/`|| true`), which fail under Visual Studio/MSBuild on Windows.
#
# The ExternalProject source may live below a different parent Git worktree
# (Hab's build directory, for example). Prevent Git from discovering that
# parent repository, otherwise `git apply` silently skips paths that are not in
# the parent worktree.
get_filename_component(source_parent "${SOURCE_DIR}" DIRECTORY)
set(git_apply_command
    "${CMAKE_COMMAND}" -E env
    "GIT_CEILING_DIRECTORIES=${source_parent}"
    "${GIT_EXECUTABLE}" apply)

execute_process(
    COMMAND ${git_apply_command} --check "${PATCH_FILE}"
    WORKING_DIRECTORY "${SOURCE_DIR}"
    RESULT_VARIABLE check_result
    OUTPUT_QUIET
    ERROR_QUIET
)

if(check_result EQUAL 0)
    execute_process(
        COMMAND ${git_apply_command} "${PATCH_FILE}"
        WORKING_DIRECTORY "${SOURCE_DIR}"
        RESULT_VARIABLE apply_result
    )
    if(NOT apply_result EQUAL 0)
        message(FATAL_ERROR "Failed to apply ${PATCH_FILE}")
    endif()
else()
    message(STATUS "Arrow patch already applied or not required: ${PATCH_FILE}")
endif()
