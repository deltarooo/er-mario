#!/usr/bin/env bash
# Builds the mod DLL on Linux by cross-compiling for Windows (the Linux counterpart of build.ps1,
# see "Building" in README.md). With --dist, also puts it into an ER-Mario folder, e.g.
#   ./build.sh --dist ~/Documents/ER-Mario
# Needs: Rust (rustup), cargo-xwin (`cargo install --locked cargo-xwin`, it downloads the MSVC CRT
# and Windows SDK on first use), clang and llvm (clang-cl and llvm-lib), e.g.
#   sudo apt install clang llvm
# and lld (lld-link) if Rust didn't come from rustup.
set -euo pipefail

usage() { echo "usage: $0 [--dist <ER-Mario folder>]" >&2; exit 2; }

# the ER-Mario folder the game loads the mod from (optional)
dist=""
while [ $# -gt 0 ]; do
    case "$1" in
        -d|--dist|-Dist) [ $# -ge 2 ] || usage; dist="$2"; shift 2 ;;
        --dist=*) dist="${1#*=}"; shift ;;
        -h|--help) usage ;;
        *) usage ;;
    esac
done

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target=x86_64-pc-windows-msvc
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
export PATH="$cargo_home/bin:$PATH"

# Debian/Ubuntu only put versioned names (clang-cl-19, lld-link-17) on PATH: the plain names live
# in /usr/lib/llvm-N/bin. Newest first, older ones after it for tools only they have (e.g. lld-link
# when just an older lld is installed).
for d in $(ls -d /usr/lib/llvm-*/bin 2>/dev/null | sort -t- -k2 -n -r); do
    PATH="$PATH:$d"
done
newest_llvm="$(ls -d /usr/lib/llvm-*/bin 2>/dev/null | sort -t- -k2 -n -r | head -n1 || true)"
[ -n "$newest_llvm" ] && PATH="$newest_llvm:$PATH"
export PATH

missing=()
command -v cargo >/dev/null || missing+=("cargo (https://rustup.rs)")
cargo xwin --version >/dev/null 2>&1 || missing+=("cargo-xwin (cargo install --locked cargo-xwin)")
command -v clang-cl >/dev/null || missing+=("clang-cl (sudo apt install clang)")
# (the linker: lld-link, or the rust-lld that comes with a rustup toolchain, which cargo-xwin
# falls back to)
command -v lld-link >/dev/null || [ -n "$(find "$(rustc --print sysroot 2>/dev/null)/lib/rustlib" -name rust-lld -type f 2>/dev/null | head -n1)" ] \
    || missing+=("lld-link (sudo apt install lld)")
command -v llvm-lib >/dev/null || missing+=("llvm-lib (sudo apt install llvm)")
if [ ${#missing[@]} -gt 0 ]; then
    echo "missing build tools:" >&2
    printf '  %s\n' "${missing[@]}" >&2
    exit 1
fi
if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
    (cd "$root" && rustup target add "$target")
fi

# libsm64 is decompiled N64 C code: clang-cl handles it, as on Windows. cargo-xwin appends its own
# include paths to these, but doesn't see .cargo/config.toml's [env] CFLAGS, so repeat them here.
export CC_x86_64_pc_windows_msvc=clang-cl
export CFLAGS_x86_64_pc_windows_msvc="-Wno-error=implicit-function-declaration -Wno-implicit-function-declaration"

# no local paths (user name) in the DLL's panic messages
flags=("--remap-path-prefix=$(dirname "$root")=src" "--remap-path-prefix=$HOME=~")
reg="$(ls -d "$cargo_home"/registry/src/*/ 2>/dev/null | head -n1 || true)"
if [ -n "$reg" ]; then
    flags=("--remap-path-prefix=${reg%/}=crates" "${flags[@]}")
fi
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="${flags[*]}"

(cd "$root" && cargo xwin build --release --target "$target")

dll="$root/target/$target/release/er_mario.dll"
if [ -z "$dist" ]; then
    echo "built $dll"
    exit 0
fi
# swap in by rename: overwriting the DLL a running game (under Proton/Wine) has loaded in place
# crashes it. A rename replaces the name only: the running game keeps the old file, the next
# start loads the new one.
cp -f "$dll" "$dist/er_mario.dll.new"
mv -f "$dist/er_mario.dll.new" "$dist/er_mario.dll"
echo "built and installed into $dist"
