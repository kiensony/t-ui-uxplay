# t-ui-uxplay

A music-only AirPlay receiver for the terminal. Select it from Apple Music’s AirPlay audio menu to play through your computer’s speakers and see the track, cover art, and lyrics in the terminal.

## Install

Release archives for Linux x64, Windows x64, and Intel and Apple Silicon Macs include UxPlay and libplist. Keep the `uxplay` folder beside the player executable and install the runtime dependencies for your system below. The folder also includes the source, licenses, and build script. [GitHub Releases](https://github.com/kiensony/t-ui-uxplay/releases)

After installing the dependencies for your system, extract the archive and run `./t-ui-uxplay` on Linux or macOS, or `.\t-ui-uxplay.exe` in PowerShell.

### Linux

On Ubuntu 22.04 or Debian 12:

```sh
sudo apt install libssl3 libavahi-compat-libdnssd1 avahi-daemon avahi-utils \
  gstreamer1.0-libav gstreamer1.0-plugins-good gstreamer1.0-plugins-bad coreutils
```

Run the player as your desktop user so GStreamer can reach your PipeWire, PulseAudio, or ALSA output. Avahi must be running, and the Linux desktop and sender must be on a network that allows mDNS discovery and AirPlay traffic. The `avahi-browse` utility from `avahi-utils` resolves senders that advertise DACP controls.

If your distribution ships different shared library versions, install its UxPlay package and use `--uxplay uxplay`. [UxPlay for Linux](https://github.com/FDH2/UxPlay/blob/v1.73.7/README.md#linux)

### macOS

Install the official GStreamer 1.28 macOS Universal runtime package and run `brew install openssl@3` using the standard Homebrew prefix for your Mac. [GStreamer downloads](https://gstreamer.freedesktop.org/download/#macos)

### Windows

Extract the entire Windows ZIP and run `t-ui-uxplay.exe` in Windows Terminal. The archive includes the UxPlay audio runtime, including GStreamer, OpenSSL, and the MinGW DLLs.

The Windows archive uses a pinned UxPlay 1.74 development build with built-in mDNS. It advertises AirPlay on an IPv4 interface with a gateway.

#### Windows DLL sources

The Windows build copies these DLLs from MSYS2 UCRT64 packages.

| Bundled DLL | MSYS2 UCRT64 package | Source recipe |
| --- | --- | --- |
| `libgcc_s_seh-1.dll` | `mingw-w64-ucrt-x86_64-libgcc` | [GCC](https://github.com/msys2/MINGW-packages/tree/master/mingw-w64-gcc) |
| `libstdc++-6.dll` | `mingw-w64-ucrt-x86_64-libstdc++` | [GCC](https://github.com/msys2/MINGW-packages/tree/master/mingw-w64-gcc) |
| `libwinpthread-1.dll` | `mingw-w64-ucrt-x86_64-libwinpthread` | [MinGW-w64 winpthreads](https://github.com/msys2/MINGW-packages/tree/master/mingw-w64-winpthreads) |
| `libcrypto-4-x64.dll` | `mingw-w64-ucrt-x86_64-openssl` | [OpenSSL](https://github.com/msys2/MINGW-packages/tree/master/mingw-w64-openssl) |

Each Windows ZIP includes `uxplay/runtime-licenses/MSYS2-runtime-sources.txt`. It lists the exact package versions, licenses, and source archive URLs for the bundled MSYS2 runtime, including GStreamer and its dependencies.

### Build from source

Build with Rust 1.88 or newer:

```sh
cargo install --locked --path .
t-ui-uxplay --name "Living Room"
```

Use `--uxplay /path/to/uxplay` to select a binary explicitly. Otherwise, the player tries the bundled copy first and then looks for `uxplay` on `PATH`. External copies use `stdbuf` on Linux or `gstdbuf` from Homebrew coreutils on macOS. The default receiver name is `t-ui-uxplay`.

On Windows, a custom UxPlay executable needs UTF-8 command-line support for Unicode speaker names and unbuffered logs to confirm renames. The bundled executable includes both.

## Connect and control playback

1. Open Apple Music on your iPhone. From Now Playing, tap AirPlay and select the receiver in the audio device list.
2. Start a song. Album art uses Kitty graphics in Kitty and Ghostty, colored character cells in other terminals, and a text layout when the window is narrow.
3. Use the controls below the artwork or track details. They become available when the sender’s local DACP service resolves.

| Key | Action |
| --- | --- |
| `p` | Previous |
| `Space` | Play/Pause |
| `n` | Next |
| `d` | Toggle Discord |
| `v` | Cycle through Cover, Ribbons, and Spectrum |
| `s` | Open Settings |
| `↑` / `↓` | Scroll lyrics |
| `[` / `]` | Delay or advance lyrics by 250 ms |
| `q` | Quit and stop the receiver |

The Ribbons preset uses the live AirPlay audio to rotate and warp neon trails. Spectrum shows 24 frequency bands with falling bars and peak markers. Press `v` to change the view for the current run.

## Settings

Click **Settings** or press `s`. Use `Tab` and `Shift+Tab` to move between fields, `Enter` to activate a field, and `Esc` to discard the draft. Enter a speaker name of up to 50 UTF-8 bytes. Choose **Save** to apply your changes. A rename restarts AirPlay and asks for confirmation if a sender is connected. Then reconnect using the new name.

**Color palette** starts on **Default**. Click the field or press `Enter` to cycle through **Femboy** (soft pink and light blue), **Transgirl** (deep pink and royal blue), and **Default**. The player and visualizers preview each choice. Choose **Save** to use it in future sessions.

**Online lyrics** starts On and **Default view** starts on Cover. Settings are stored in `t-ui-uxplay/settings.json` under `$XDG_CONFIG_HOME` (or `~/.config`) on Linux, `~/Library/Application Support` on macOS, and `%APPDATA%` on Windows. `--name`, `--no-online-lyrics`, and `--discord-app-id` override and lock their corresponding fields for that launch.

LRCLIB lookups send the title, artist, album, and available duration. If the first result has line timings, the player also checks for word timings on a release with the same song, artist, and a similar duration. Results are cached until the player quits. Turn **Online lyrics** off to clear the panel and stop lookups.

The lyrics panel shows **Word sync** when the words can light up progressively as they’re sung and **Line sync** when only line timings are available. UxPlay reports whole-second progress. The player compensates for rounding, estimates playback between updates, and holds the estimate when updates stop. If lyrics still lag, press `]` to advance them by 250 ms. Press `[` to move them later. The adjustment lasts for the current run. With no timing, lyrics stay in a scrollable text view. After scrolling synced lyrics, automatic following resumes after eight seconds. [LRCLIB lyrics formats](https://lrclib.net/docs#lyrics-record-fields)

## Discord Rich Presence

Create an application in the Discord Developer Portal and copy its **Application ID**. In the receiver, open **Settings** with `s`, enter the ID in **Discord Application ID**, and choose **Save**. If sharing is active, the receiver switches to the new Discord application. Clear the field and save to remove the saved ID. Start the signed-in Discord desktop app on the same computer. [Discord Developer Portal](https://discord.com/developers/applications)

To override the saved ID for one launch, pass it on the command line.

```sh
t-ui-uxplay --discord-app-id YOUR_APPLICATION_ID
```

Sharing starts **Off** each launch. Click **Discord** or press `d` to enable it. **Sharing** means Discord IPC is connected. While a track is available, the activity is **Listening** and includes the title and artist. **Unavailable** means the app could not publish or reach Discord, or no application ID was configured. The player retries IPC while enabled. It clears the activity when sharing is turned off, the sender disconnects, or the player quits.

Discord uses the song title in **Listening to**. When AirPlay sends track timing, the activity card shows a live progress bar with elapsed time and total track length, such as `1:20 / 3:30`. The bar updates after a seek. After three seconds without progress, the card shows the last reported time as text until playback advances again. If AirPlay provides no timing for the track, the card shows the song and artist.

## Verify

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Before release, stream Apple Music from an iPhone on the same network. On Linux, macOS, and Windows, rename the speaker from Settings, reconnect under the new name, and use Previous, Play/Pause, and Next. Verify audio playback, track changes, artwork, pause and seek alignment, and cleanup when the sender disconnects. Check artwork in a Kitty graphics terminal and a fallback terminal.

With a Discord application ID configured, view the Listening activity from another Discord account. Verify the song title, live progress bar, and elapsed and total times. Seek in both directions, pause for at least three seconds, resume, and switch tracks. Verify the activity clears when sharing is turned off, the sender disconnects, and the receiver quits.

## GitHub Actions

Pull requests and pushes to `main` run the Rust checks and upload a release archive for each platform to the workflow run. You can also run **Build and release** manually from the Actions tab. To publish all four archives to GitHub Releases, push a version tag such as `v0.1.0` after the workflow is on `main`.
