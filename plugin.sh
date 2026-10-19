#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/grok-sub.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/grok-sub" ;;
    *) rel="x86_64-apple-darwin/grok-sub" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/grok-sub" ;;
    *) rel="x86_64-unknown-linux-musl/grok-sub" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/grok-sub.exe" "$here/target/release/grok-sub" "$here/target/debug/grok-sub.exe" "$here/target/debug/grok-sub"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "grok-sub binary not found in $here" >&2
exit 1
