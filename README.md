# Antifa

Antifa is a desktop Tor messenger written in Rust with `eframe/egui` for the UI and Arti for the Tor transport layer.

Website: https://uintell.org

Each running instance boots Tor, publishes its own `.onion` address, and can open a direct session with another instance by connecting to that peer's `.onion`.

## What it does

- boots a local Tor client inside the app
- creates a local onion service for inbound messages
- lets you copy your own `.onion` address from the UI
- connects to another peer by `.onion`
- sends direct text messages over Tor
- streams experimental low-bandwidth video frames over Tor

## Project layout

- `src/main.rs`: app entrypoint
- `src/app.rs`: egui desktop interface and user interaction flow
- `src/p2p.rs`: Tor transport, onion service, peer connection, message send/receive
- `src/tor.rs`: Tor status event types
- `src/messaging.rs`: chat message models and timestamps
- `.cargo/config.toml`: Windows cross-compile linker config
- `scripts/build-windows.sh`: helper script for Windows builds

## Requirements

### Linux

- Rust toolchain with Cargo
- the native libraries needed by `eframe`/`wgpu`
- `ffmpeg` available in `PATH`

### Windows cross-build from Linux

- Rust target `x86_64-pc-windows-gnu`
- `x86_64-w64-mingw32-gcc` available in `PATH`
- `ffmpeg` available on the Windows machine where you run the app

The repo is already configured to use `x86_64-w64-mingw32-gcc` through `.cargo/config.toml`.

## Run locally

Build:

```bash
cargo build
```

Run:

```bash
cargo run
```

When the app starts, it will bootstrap Tor automatically. Wait until the status changes from bootstrapping/publishing to a ready/reachable state before trying to connect to another peer.

## Build for Windows

Install the Windows target once:

```bash
rustup target add x86_64-pc-windows-gnu
```

Build manually:

```bash
cargo build --release --target x86_64-pc-windows-gnu
```

Or use the helper script:

```bash
./scripts/build-windows.sh
```

The Windows executable will be generated under:

```text
target/x86_64-pc-windows-gnu/release/antifa.exe
```

## How to use the app

1. Start the app on both sides.
2. Wait for your local Tor status to finish bootstrapping and for the onion service to become reachable.
3. In the `Identity` section, copy your `.onion` address with `Copy onion`.
4. Share that `.onion` address with the person you want to talk to.
5. Paste their `.onion` into `Friend .onion`.
6. Optionally set a label in the `Label` field.
7. Click `Connect`.
8. Type a message and press `Send` or `Enter`.

## Experimental video over Tor

This repo now includes an experimental video mode. It is not WebRTC and it does not try to provide smooth real-time video; it sends low-bandwidth PNG frames over a dedicated Tor stream.

Important limits:

- expect high latency
- expect a low frame rate
- both peers need to press `Start Video` if they want two-way video

How to use it:

1. Start a normal text connection first or at least fill in the peer's `.onion`.
2. Click `Start Video`.
3. Wait for the remote preview to start updating.
4. If both sides want video, both sides should start video.

Linux camera source:

- by default the app uses `/dev/video0`
- to use a different camera, set `ANTIFA_VIDEO_DEVICE`

Example:

```bash
ANTIFA_VIDEO_DEVICE=/dev/video2 cargo run
```

Windows camera source:

- set `ANTIFA_VIDEO_INPUT` to your DirectShow camera name before starting the app

Example:

```powershell
$env:ANTIFA_VIDEO_INPUT = "video=Integrated Camera"
cargo run
```

## Notes

- the app normalizes pasted onion addresses, so `Example.Onion` and `example.onion/` are both accepted
- chat state is currently in-memory only; there is no persistent message history in this repo
- the transport uses an internal onion service port of `17600`; users do not need to configure this manually
- video capture depends on `ffmpeg` and is intentionally conservative to keep bandwidth lower over Tor

## GitHub

This repository intentionally keeps only the working source project and Windows build support:

- source: `src/`
- Rust manifest and lockfile: `Cargo.toml`, `Cargo.lock`
- Windows build support: `.cargo/config.toml`, `scripts/build-windows.sh`

Generated build output and old project copies are excluded from git with `.gitignore`.
