# t-ui-uxplay

A music-only AirPlay receiver for the terminal. Select it from Apple Music’s AirPlay audio menu to play through your computer’s speakers and see the track, cover art, and lyrics in the terminal.

## Install

Releases include archives for Linux x64, Windows x64, and Intel and Apple Silicon Macs. Install UxPlay and its GStreamer audio dependencies separately. UxPlay 1.73.7 is the reference version. The packaged 1.73 build also supports the required options and export formats. [GitHub Releases](https://github.com/kiensony/t-ui-uxplay/releases)

After installing the dependencies for your system, extract the archive and run `./t-ui-uxplay` on Linux or macOS, or `.\t-ui-uxplay.exe` in PowerShell.

### Linux

On Debian or Ubuntu, where UxPlay is packaged:

```sh
sudo apt install uxplay avahi-daemon avahi-utils gstreamer1.0-libav gstreamer1.0-plugins-good coreutils
```

Run the player as your desktop user so GStreamer can reach your PipeWire, PulseAudio, or ALSA output. Avahi must be running, and the Linux desktop and sender must be on a network that allows mDNS discovery and AirPlay traffic. The `avahi-browse` utility from `avahi-utils` resolves senders that advertise DACP controls.

For other distributions, see the upstream installation instructions. [UxPlay for Linux](https://github.com/FDH2/UxPlay/blob/v1.73.7/README.md#linux)

### macOS

Install GNU coreutils with `brew install coreutils` so `gstdbuf` is available on `PATH`. Follow the upstream instructions to install UxPlay and GStreamer. [UxPlay for macOS](https://github.com/FDH2/UxPlay/blob/v1.73.7/README.md#building-uxplay-on-macos-intel-x86_64-and-apple-silicon-m1m2-macs)

### Windows

Follow the upstream Windows instructions to install UxPlay, Bonjour, and the GStreamer plugins. Launch `t-ui-uxplay.exe` in Windows Terminal with UxPlay and its dependency DLLs on `PATH`. [UxPlay for Windows](https://github.com/FDH2/UxPlay/blob/v1.73.7/README.md#building-uxplay-on-microsoft-windows-using-msys2-with-the-mingw-64-compiler)

Stock Windows builds of UxPlay can leave connection status and Discord activity stale until UxPlay flushes its logs. If you build UxPlay from source, add `setvbuf(stdout, nullptr, _IONBF, 0);` at the start of `main` in `uxplay.cpp` to make status updates immediate. [UxPlay source](https://github.com/FDH2/UxPlay/blob/v1.73.7/uxplay.cpp#L221-L241)

### Build from source

Build with Rust 1.88 or newer:

```sh
cargo install --locked --path .
t-ui-uxplay --name "Living Room"
```

Use `--uxplay /path/to/uxplay` to select a binary. The default receiver name is `t-ui-uxplay`, and the default executable is `uxplay` on `PATH`. The player starts UxPlay with its own arguments and uses the default GStreamer audio sink.

## Connect and control playback

1. Open Apple Music on your iPhone. From Now Playing, tap AirPlay and select the receiver in the audio device list.
2. Start a song. Album art uses Kitty graphics in Kitty and Ghostty, colored character cells in other terminals, and a text layout when the window is narrow.
3. Click the buttons or use the keys below. Transport buttons become available when the sender’s local DACP service resolves.

| Key | Action |
| --- | --- |
| `p` | Previous |
| `Space` | Play/Pause |
| `n` | Next |
| `d` | Toggle Discord |
| `v` | Cycle through Cover, Ribbons, and Spectrum |
| `↑` / `↓` | Scroll lyrics |
| `[` / `]` | Delay or advance lyrics by 250 ms |
| `q` | Quit and stop the receiver |

The Ribbons preset uses the live AirPlay audio to rotate and warp neon trails. Spectrum shows 24 frequency bands with falling bars and peak markers. Press `v` to cycle through both presets and the album cover.

LRCLIB lookups send the title, artist, album, and available duration. If the first result has line timings, the player also checks for word timings on a release with the same song, artist, and a similar duration. Results are cached until the player quits. Use `--no-online-lyrics` to disable lookups.

The lyrics panel shows **Word sync** when the words can light up progressively as they’re sung and **Line sync** when only line timings are available. UxPlay reports whole-second progress. The player compensates for rounding, estimates playback between updates, and holds the estimate when updates stop. If lyrics still lag, press `]` to advance them by 250 ms. Press `[` to move them later. The adjustment lasts for the current run. With no timing, lyrics stay in a scrollable text view. After scrolling synced lyrics, automatic following resumes after eight seconds. [LRCLIB lyrics formats](https://lrclib.net/docs#lyrics-record-fields)

## Discord Rich Presence

Create an application in the [Discord Developer Portal](https://discord.com/developers/applications) and copy its **Application ID**. Start the signed-in Discord desktop app on the same computer. Pass the application ID when starting the receiver.

```sh
t-ui-uxplay --discord-app-id YOUR_APPLICATION_ID
```

Sharing starts **Off** each launch. Click **Discord** or press `d` to enable it. **Sharing** means Discord IPC is connected. While a track is available, the activity is **Listening** and includes the title and artist. **Unavailable** means the app could not publish or reach Discord, or no application ID was configured. The player retries IPC while enabled. It clears the activity when sharing is turned off, the sender disconnects, or the player quits.

Discord uses the song title in **Listening to**. When AirPlay sends track timing, the activity card shows a progress bar that updates after a seek. After three seconds without progress, the card shows the last elapsed time and track length as text until playback advances again.

## Verify

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Before release, stream Apple Music from an iPhone on the same network. Verify audio playback, track changes, artwork, pause and seek alignment, and cleanup when the sender disconnects. Check artwork in a Kitty graphics terminal and a fallback terminal. With your actual Discord application ID, verify that the Listening activity appears, updates with the track, and clears on disconnect, toggle off, and quit.

## GitHub Actions

Pull requests and pushes to `main` run the Rust checks and upload a release archive for each platform to the workflow run. You can also run **Build and release** manually from the Actions tab. To publish all four archives to GitHub Releases, push a version tag such as `v0.1.0` after the workflow is on `main`.
