#!/bin/sh
# Build Skyward for a Realme Neo7 (RMX5060) and write it into the phone's
# vendor_boot, where the phone keeps its recovery.
#
# On a computer: the Rust toolchain rust-toolchain.toml names with the
# aarch64-linux-android target, Android NDK r29 and cargo-ndk, lz4, and the
# phone's own vendor_boot image — the whole partition as the phone holds it,
# ending in its AVB footer, which is what `fastboot fetch vendor_boot` or a
# full update package gives.
#
#   STOCK=vendor_boot.img sh devices/rmx5060/build.sh
#
# Writes target/vendor_boot_skyward.img, to be flashed over vendor_boot on
# an unlocked phone whose verification is off; see docs/rmx5060.md.
set -e
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

stock=${STOCK:?give the phone's own vendor_boot image in STOCK}
[ -f "$stock" ] || { echo "no $stock" >&2; exit 1; }

echo "building Skyward for the phone"
cargo ndk -t arm64-v8a --platform 30 build --release -p skyward

echo "building the second system's first program"
RUSTFLAGS="-C target-feature=+crt-static" \
    cargo ndk -t arm64-v8a --platform 30 build --release -p skyward-system-init --target-dir target/system-init

echo "building the packer"
cargo build --release -p skyward-pack

echo "writing the recovery into a vendor_boot of its own"
target/release/skyward-pack vendor-boot \
    --stock "$stock" \
    --binary target/aarch64-linux-android/release/skyward \
    --system-init target/system-init/aarch64-linux-android/release/system-init \
    --config devices/rmx5060.toml \
    --manifest devices/rmx5060.recovery.toml \
    --init-rc crates/skyward-pack/recovery/init.rc \
    --usb-rc crates/skyward-pack/recovery/usb.rc \
    --font devices/rmx5060/MiSansVF-subset.ttf \
    --mono resources/fonts/KirkMono-Regular.ttf \
    --mono-license resources/fonts/KirkMono-OFL.txt \
    --out target/vendor_boot_skyward.img

echo
echo "flash it with:"
echo "  fastboot --disable-verity --disable-verification flash vbmeta vbmeta.img"
echo "  fastboot flash vendor_boot target/vendor_boot_skyward.img"
echo "  fastboot reboot recovery"
echo "and go back with: fastboot flash vendor_boot $stock"
