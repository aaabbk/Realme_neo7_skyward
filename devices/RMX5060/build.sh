#!/bin/sh
# Build Skyward for a Realme Neo7 (RMX5060) and write it into the phone's
# vendor_boot, where the phone keeps its recovery.
#
# On a computer: the Rust toolchain rust-toolchain.toml names with the
# aarch64-linux-android target, an Android NDK (its clang links the phone's
# programs), lz4, and the phone's own vendor_boot image — the whole
# partition as the phone holds it, ending in its AVB footer, which is what
# `fastboot fetch vendor_boot` or a full update package gives.
#
#   ANDROID_NDK_HOME=/path/to/android-ndk STOCK=vendor_boot.img \
#       sh devices/RMX5060/build.sh
#
# Writes target/vendor_boot_skyward.img, to be flashed over vendor_boot on
# an unlocked phone whose verification is off; see docs/RMX5060.md.
set -e
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

stock=${STOCK:?give the phone its own vendor_boot image in STOCK}
ndk=${ANDROID_NDK_HOME:-${NDK:?give the folder of an Android NDK in ANDROID_NDK_HOME}}
[ -f "$stock" ] || { echo "no $stock" >&2; exit 1; }

# The NDK's own folder for this computer, whichever it is: the same clang
# builds the phone's programs on a computer of the phone's own kind.
host=$(basename "$(ls -d "$ndk"/toolchains/llvm/prebuilt/*/ | head -1)")
bin=$ndk/toolchains/llvm/prebuilt/$host/bin
api=${API:-30}

export CC_aarch64_linux_android=$bin/aarch64-linux-android$api-clang
export CXX_aarch64_linux_android=$bin/aarch64-linux-android$api-clang++
export AR_aarch64_linux_android=$bin/llvm-ar
export RANLIB_aarch64_linux_android=$bin/llvm-ranlib
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$CC_aarch64_linux_android

echo "building Skyward for the phone"
cargo build --release -p skyward --target aarch64-linux-android

echo "building the second system's first program"
RUSTFLAGS="-C target-feature=+crt-static" \
    cargo build --release -p skyward-system-init --target aarch64-linux-android --target-dir target/system-init

echo "building the packer"
cargo build --release -p skyward-pack

echo "writing the recovery into a vendor_boot of its own"
target/release/skyward-pack vendor-boot \
    --stock "$stock" \
    --binary target/aarch64-linux-android/release/skyward \
    --system-init target/system-init/aarch64-linux-android/release/system-init \
    --config devices/RMX5060.toml \
    --manifest devices/RMX5060.recovery.toml \
    --init-rc crates/skyward-pack/recovery/init.rc \
    --usb-rc crates/skyward-pack/recovery/usb.rc \
    --font devices/RMX5060/MiSansVF-subset.ttf \
    --mono resources/fonts/KirkMono-Regular.ttf \
    --mono-license resources/fonts/KirkMono-OFL.txt \
    --out target/vendor_boot_skyward.img

echo
echo "flash it with:"
echo "  fastboot --disable-verity --disable-verification flash vbmeta vbmeta.img"
echo "  fastboot flash vendor_boot target/vendor_boot_skyward.img"
echo "  fastboot reboot recovery"
echo "and go back with: fastboot flash vendor_boot $stock"
