#!/usr/bin/env bash
# Builds whisper.cpp's command-line tools and places them where Tauri expects its sidecars.
#
#   scripts/build-whisper-mac.sh [target-triple]
#
# Produces app/src-tauri/binaries/whisper-cli-<triple> and whisper-quantize-<triple>.
# The build is static (no dylibs to ship) and, on Intel, CPU-only: ggml's Metal backend
# needs Apple Silicon, so it is left out of the x86_64 build.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
version="${WHISPER_CPP_VERSION:-v1.9.4}"
case "$triple" in
  # A 2019 MacBook Pro (Coffee Lake) has AVX2/FMA/F16C; ggml must be told to use them
  # when it isn't tuning for the build machine.
  aarch64-apple-darwin) arch=arm64; metal=ON; cpu_flags="" ;;
  x86_64-apple-darwin) arch=x86_64; metal=OFF; cpu_flags="-DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON" ;;
  *) echo "unsupported target: $triple" >&2; exit 1 ;;
esac

src="$root/.build/whisper.cpp"
if [ ! -d "$src/.git" ]; then
  git clone --depth 1 --branch "$version" https://github.com/ggml-org/whisper.cpp.git "$src"
fi
build="$src/build-$arch"
cmake -S "$src" -B "$build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES="$arch" \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=14.2 \
  -DBUILD_SHARED_LIBS=OFF \
  -DGGML_METAL="$metal" \
  -DGGML_METAL_EMBED_LIBRARY=ON \
  -DGGML_NATIVE=OFF $cpu_flags \
  -DWHISPER_BUILD_TESTS=OFF \
  -DWHISPER_BUILD_SERVER=OFF
cmake --build "$build" --config Release --target whisper-cli whisper-quantize -j "$(sysctl -n hw.ncpu)"

out="$root/app/src-tauri/binaries"
mkdir -p "$out"
cp "$build/bin/whisper-cli" "$out/whisper-cli-$triple"
cp "$build/bin/whisper-quantize" "$out/whisper-quantize-$triple"
# Make sure nothing links against a dylib we don't ship.
if otool -L "$out/whisper-cli-$triple" | grep -q "libwhisper\|libggml"; then
  echo "whisper-cli is dynamically linked; expected a static build" >&2; exit 1
fi
echo "built $out/whisper-cli-$triple and whisper-quantize-$triple"
