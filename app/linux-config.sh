#!/bin/bash
set -e # Quit script on error
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
WORKING_DIR="$(pwd)"

cleanup_function() {
    # Restore working directory as it was prior to this script running on exit
    cd "${WORKING_DIR}"
}
trap cleanup_function EXIT

args=("$@")
config="Release"

# extract options and their arguments into variables.
while [ -n "$1" ]; do
    case "$1" in
        -c|--config)
            case $2 in
                Release|Debug|RelWithDebInfo|MinSizeRel)
                    config="$2"
                    ;;
                *)
                    echo "`basename $0`: Error: invalid configuration: '${2}'" 1>&2
                    echo "Valid configurations: Release, Debug, RelWithDebInfo, MinSizeRel" 1>&2
                    exit 1
                    ;;
            esac
            shift 2
            ;;
        --) shift ; break ;;
        *) echo "Invalid argument: $1" ; exit 1 ;;
    esac
done

# ruby_help.h is generated (docs -> C header) and cmake fails with a cryptic
# "Cannot find source file" without it. Self-heal instead of assuming the
# reader knows to run linux-pre-translations.sh first.
if [ ! -f "${SCRIPT_DIR}/gui/utils/ruby_help.h" ]; then
    echo "gui/utils/ruby_help.h missing — generating it (linux-pre-translations.sh)..."
    "${SCRIPT_DIR}/linux-pre-translations.sh"
fi

echo "Creating build directory..."
mkdir -p "${SCRIPT_DIR}/build"

echo "Generating makefiles..."
cd "${SCRIPT_DIR}/build"

cmake -G "Unix Makefiles" -DCMAKE_BUILD_TYPE="$config" ..

