#!/bin/sh
set -eu

REPOSITORY_ROOT="${SRCROOT}/.."
USER_DIRECTORY="${CFFIXED_USER_HOME:-/Users/$(id -un)}"
CARGO_BIN="${CARGO_HOME:-${USER_DIRECTORY}/.cargo}/bin/cargo"

if [ ! -x "${CARGO_BIN}" ]; then
    echo "error: cargo was not found at ${CARGO_BIN}" >&2
    exit 1
fi

case "${PLATFORM_NAME}" in
    iphoneos)
        RUST_TARGET="aarch64-apple-ios"
        ;;
    iphonesimulator)
        RUST_TARGET="aarch64-apple-ios-sim"
        ;;
    *)
        echo "error: unsupported Xcode platform ${PLATFORM_NAME}" >&2
        exit 1
        ;;
esac

cd "${REPOSITORY_ROOT}"

if [ "${CONFIGURATION}" = "Debug" ]; then
    "${CARGO_BIN}" build --lib --target "${RUST_TARGET}"
else
    "${CARGO_BIN}" build --release --lib --target "${RUST_TARGET}"
fi
