#!/usr/bin/env bash
set -euo pipefail

destination=${1:?Provide the destination directory for UxPlay}
script_dir=$(cd "$(dirname "$0")" && pwd)
platform=${RUNNER_OS:-$(uname -s)}
cmake_options=()
executable=uxplay

case "$platform" in
  Linux|macOS|Darwin)
    revision=df67c212a433cf6dda3676dd40c097900d24e645 # v1.73.7
    checksum=168de53ed688f87c4022d79357ce91f9608ad5c53236bff95134d19b88efeb3f
    ;;
  Windows|UCRT64_NT-*)
    # This revision implements mDNS without the Bonjour SDK on Windows.
    revision=48f5ce47c9ef694afccfac907c9e8828329657fe # 1.74 development snapshot
    checksum=c8d95e30e6f910774dc162fc65ce4df47cc4bfdc7f44733b1aa2e7334b8869ba
    executable=uxplay.exe
    cmake_options+=(-DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++ -DUSE_MDNS=ON)
    ;;
  *) printf 'Unsupported UxPlay build platform %s\n' "$platform" >&2; exit 1 ;;
esac

workspace=$(mktemp -d)
trap 'rm -rf "$workspace"' EXIT
curl --fail --show-error --silent --location --retry 3 \
  "https://codeload.github.com/FDH2/UxPlay/tar.gz/$revision" -o "$workspace/upstream.tar.gz"
if [[ "$platform" == macOS || "$platform" == Darwin ]]; then
  printf '%s  %s\n' "$checksum" "$workspace/upstream.tar.gz" | shasum -a 256 --check
  # Use the installed shared libraries in place of UxPlay's default static libraries.
  cmake_options+=(
    -DCMAKE_OSX_DEPLOYMENT_TARGET=11.0
    "-DLIBPLIST=$(brew --prefix libplist)/lib/libplist-2.0.dylib"
    "-DLIBCRYPTO=$(brew --prefix openssl@3)/lib/libcrypto.dylib"
  )
else
  printf '%s  %s\n' "$checksum" "$workspace/upstream.tar.gz" | sha256sum --check
fi

source_dir="$workspace/UxPlay-$revision"
tar -xzf "$workspace/upstream.tar.gz" -C "$workspace"
patch -t -N -F 0 -d "$source_dir" -p1 < "$script_dir/uxplay-flush-logs.patch"

cmake -S "$source_dir" -B "$workspace/build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DNO_MARCH_NATIVE=ON -DNO_X11_DEPS=ON "${cmake_options[@]}"
cmake --build "$workspace/build" --target uxplay

mkdir -p "$destination"
install -m 755 "$workspace/build/$executable" "$destination/$executable"
cp "$source_dir/LICENSE" "$destination/LICENSE"
cp "$script_dir/bundle-uxplay.sh" "$script_dir/uxplay-flush-logs.patch" "$destination/"
tar -czf "$destination/source.tar.gz" -C "$workspace" "UxPlay-$revision"

touch "$workspace/empty.conf"
version_output=$("$destination/$executable" -rc "$workspace/empty.conf" -v)
printf '%s\n' "$version_output"
[[ "$version_output" == *"UxPlay version "* ]]
