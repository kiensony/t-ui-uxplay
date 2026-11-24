#!/usr/bin/env bash
set -euo pipefail

destination=${1:?Provide the destination directory for UxPlay}
script_dir=$(cd "$(dirname "$0")" && pwd)
platform=${RUNNER_OS:-$(uname -s)}
cmake_options=()
hash_command=(sha256sum)
executable=uxplay
revision=df67c212a433cf6dda3676dd40c097900d24e645 # v1.73.7
checksum=168de53ed688f87c4022d79357ce91f9608ad5c53236bff95134d19b88efeb3f

case "$platform" in
  Linux)
    plist_library=lib/libplist-2.0.so.12
    plist_link=lib/libplist-2.0.so
    cmake_options+=(-DCMAKE_BUILD_WITH_INSTALL_RPATH=ON '-DCMAKE_INSTALL_RPATH=$ORIGIN')
    ;;
  macOS|Darwin)
    hash_command=(shasum -a 256)
    plist_library=lib/libplist-2.0.12.dylib
    plist_link=lib/libplist-2.0.dylib
    export MACOSX_DEPLOYMENT_TARGET=11.0
    cmake_options+=("-DLIBCRYPTO=$(brew --prefix openssl@3)/lib/libcrypto.dylib")
    ;;
  Windows|UCRT64_NT-*)
    # This revision implements mDNS without the Bonjour SDK on Windows.
    revision=48f5ce47c9ef694afccfac907c9e8828329657fe # 1.74 development snapshot
    checksum=c8d95e30e6f910774dc162fc65ce4df47cc4bfdc7f44733b1aa2e7334b8869ba
    executable=uxplay.exe
    plist_library=bin/libplist-2.0.dll
    plist_link=lib/libplist-2.0.dll.a
    cmake_options+=(-DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++ -DUSE_MDNS=ON)
    ;;
  *) printf 'Unsupported UxPlay build platform %s\n' "$platform" >&2; exit 1 ;;
esac

workspace=$(mktemp -d)
trap 'rm -rf "$workspace"' EXIT

download_checked() {
  local url=$1 path=$2 expected=$3
  curl --fail --show-error --silent --location --retry 3 "$url" -o "$path"
  printf '%s  %s\n' "$expected" "$path" | "${hash_command[@]}" --check
}

plist_version=2.8.1
plist_archive="$workspace/libplist-source.tar.bz2"
download_checked \
  "https://github.com/libimobiledevice/libplist/releases/download/$plist_version/libplist-$plist_version.tar.bz2" \
  "$plist_archive" 2a1792e5820e1657c173735fde75baeebd6dd967d2ac47be1e3891ddc64ebefa
tar -xjf "$plist_archive" -C "$workspace"
plist_source="$workspace/libplist-$plist_version"
plist_install="$workspace/libplist-install"
if [[ "$executable" == uxplay.exe ]]; then
  plist_install=$(cygpath -m "$plist_install")
fi
mkdir "$workspace/libplist-build"
(
  cd "$workspace/libplist-build"
  "$plist_source/configure" --prefix="$plist_install" --disable-static \
    --without-cython --without-tools --without-tests
  make -j 4
  make install
)
if [[ "$platform" == macOS || "$platform" == Darwin ]]; then
  install_name_tool -id "@loader_path/${plist_library##*/}" "$plist_install/$plist_library"
  codesign --force --sign - "$plist_install/$plist_library"
fi

download_checked "https://codeload.github.com/FDH2/UxPlay/tar.gz/$revision" \
  "$workspace/upstream.tar.gz" "$checksum"
source_dir="$workspace/UxPlay-$revision"
tar -xzf "$workspace/upstream.tar.gz" -C "$workspace"
patch -t -N -F 0 -d "$source_dir" -p1 < "$script_dir/uxplay-flush-logs.patch"

if [[ "$executable" == uxplay.exe ]]; then
  # UxPlay reads narrow argv and must receive speaker names as UTF-8.
  cat > "$source_dir/uxplay-utf8.manifest" <<'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="UxPlay" version="1.0.0.0"/>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
</assembly>
EOF
  printf '%s\n' '1 24 "uxplay-utf8.manifest"' > "$source_dir/uxplay-utf8.rc"
  cat >> "$source_dir/CMakeLists.txt" <<'EOF'

enable_language(RC)
target_sources(uxplay PRIVATE uxplay-utf8.rc)
EOF
fi

cmake -S "$source_dir" -B "$workspace/build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DNO_MARCH_NATIVE=ON -DNO_X11_DEPS=ON \
  "-DCMAKE_PREFIX_PATH=$plist_install" "-DLIBPLIST=$plist_install/$plist_link" "${cmake_options[@]}"
cmake --build "$workspace/build" --target uxplay

mkdir -p "$destination"
install -m 755 "$workspace/build/$executable" "$destination/$executable"
cp -L "$plist_install/$plist_library" "$destination/"
cp "$plist_archive" "$destination/"
cp "$plist_source/COPYING" "$destination/libplist-LICENSE"
cp "$source_dir/LICENSE" "$destination/LICENSE"
cp "$script_dir/bundle-uxplay.sh" "$script_dir/uxplay-flush-logs.patch" "$destination/"
tar -czf "$destination/source.tar.gz" -C "$workspace" "UxPlay-$revision"

if [[ "$executable" == uxplay.exe ]]; then
  bash "$script_dir/bundle-windows-runtime.sh" "$destination"
  cp "$script_dir/bundle-windows-runtime.sh" "$destination/"
fi

touch "$workspace/empty.conf"
version_output=$("$destination/$executable" -rc "$workspace/empty.conf" -v)
printf '%s\n' "$version_output"
[[ "$version_output" == *"UxPlay version "* ]]
