#!/bin/sh
# Windows x64 build from macOS for testing on a PC: dist/windows/KERN + portable zip.
# Needs cargo-xwin (downloads the MSVC CRT and Windows SDK on first use; that
# accepts Microsoft's license) and LLVM (brew install llvm) for llvm-rc.
# The GPUI shaders are compiled at startup in this build (profile `xwin`).
set -eu
cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/llvm/bin:$PATH"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cargo xwin build --locked --profile xwin --target x86_64-pc-windows-msvc
OUT=dist/windows/KERN
rm -rf "$OUT"
mkdir -p "$OUT"
cp target/x86_64-pc-windows-msvc/xwin/kern.exe "$OUT/KERN.exe"
cp README.md "$OUT/README.md"
ZIP="dist/windows/KERN-$VERSION-x64-portable.zip"
rm -f "$ZIP"
(cd dist/windows && zip -qr "../../$ZIP" KERN)
echo "Built $ZIP"
