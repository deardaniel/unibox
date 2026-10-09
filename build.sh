#!/bin/sh
# Build Unibox. Rust and llvm-mingw live in the home directory and are not on PATH by default.
#   ./build.sh          Linux build for development (run with: target/debug/unibox --serve)
#   ./build.sh windows  release .exe at target/x86_64-pc-windows-gnullvm/release/unibox.exe
set -e
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$HOME/.local/opt/llvm-mingw/bin:$PATH"
if [ "$1" = "windows" ]; then
  export CC_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang
  export AR_x86_64_pc_windows_gnullvm=llvm-ar
  exec cargo build --release --target x86_64-pc-windows-gnullvm
fi
exec cargo build "$@"
