#!/bin/sh
# Checks out vendor/cxx-qt at the commit the repository pins and applies vendor/*.patch to it, each once.
# Every build of the Linux app runs it first: without the patches cargo still builds, but a binary that
# runs only on the Qt version it was built with.
#
#     gui/linux/vendor/prepare.sh
set -eu

root=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$root"
git submodule update --init gui/linux/vendor/cxx-qt

cd gui/linux/vendor/cxx-qt
for patch in ../*.patch; do
    if git apply --reverse --check "$patch" 2>/dev/null; then
        continue
    fi
    git apply "$patch"
done
