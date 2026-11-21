#!/usr/bin/env bash
set -euo pipefail

bundle=${1:?Provide the UxPlay bundle directory}
prefix=${MINGW_PREFIX:?Run this script from the MSYS2 UCRT64 shell}
plugins="$bundle/plugins"
notices="$bundle/runtime-licenses"
mkdir -p "$plugins" "$notices"

scan_files=("$bundle/uxplay.exe" "$bundle/libplist-2.0.dll")
runtime_sources=()
declare -A copied_dlls=()

bundle_file() {
  install -m 755 "$1" "$2/"
  scan_files+=("$1")
  runtime_sources+=("$1")
}

# Include the audio pipeline and the plugins UxPlay checks for at startup.
for plugin in app audioconvert audioresample autodetect coreelements directsound \
              level libav playback videoparsersbad volume wasapi2; do
  bundle_file "$prefix/lib/gstreamer-1.0/libgst$plugin.dll" "$plugins"
done
bundle_file "$prefix/libexec/gstreamer-1.0/gst-plugin-scanner.exe" "$bundle"
bundle_file "$prefix/bin/gst-inspect-1.0.exe" "$bundle"

# Follow PE imports through the toolchain's DLLs, including libgcc and libcrypto.
for ((index = 0; index < ${#scan_files[@]}; index++)); do
  imports=$(objdump -p "${scan_files[index]}" | awk '/DLL Name:/ { print $3 }')
  while IFS= read -r dll; do
    if [[ -n "$dll" && -f "$prefix/bin/$dll" && -z ${copied_dlls[${dll,,}]+x} ]]; then
      bundle_file "$prefix/bin/$dll" "$bundle"
      copied_dlls[${dll,,}]=1
    fi
  done <<< "$imports"
done

owners=$(pacman -Qoq -- "${runtime_sources[@]}" | sort -u)
manifest="$notices/MSYS2-runtime-sources.txt"
printf '%s\n\n' 'Each MSYS2 source archive lists the upstream sources and includes the packaging recipe and patches.' > "$manifest"
while IFS= read -r package; do
  installed=$(pacman -Q "$package")
  metadata=$(pacman -Sddp --print-format '%e %v %L' "$package")
  read -r base version licenses <<< "$metadata"
  [[ -n "$base" && "$version" == "${installed#* }" ]]
  printf '%s\n%s\nhttps://mirror.msys2.org/mingw/sources/%s-%s.src.tar.zst\n\n' \
    "$installed" "$licenses" "$base" "${version#*:}" >> "$manifest"

  files=$(pacman -Qlq "$package")
  while IFS= read -r path; do
    if [[ "$path" == "$prefix/share/licenses/"* && -f "$path" ]]; then
      relative=${path#"$prefix/share/licenses/"}
      install -Dm 644 "$path" "$notices/$relative"
    fi
  done <<< "$files"
done <<< "$owners"
