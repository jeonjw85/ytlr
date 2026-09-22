# YTLiveRecord

YTLiveRecord is a durable live-stream recording application for macOS, Linux,
and Windows. It keeps source media when a stream reconnects or a service is
restarted, and provides both a Tauri desktop app and a command-line client.

The project is currently at **0.2.0** and is in stabilization testing.

## Highlights

- Record a live URL, wait for a scheduled stream, or monitor a channel.
- Select the best stream available through `yt-dlp` and record without
  re-encoding through FFmpeg.
- Preserve source fragments and 30-second MKV segments across reconnects.
- Detect reception stalls, record events, and expose diagnostic logs.
- Recover playable results from source tracks or completed segments.
- Export compatible results to MP4 without replacing the original files.
- Verify media containers and audio/video headers before marking a recording
  complete.
- Back up finalized source files to a separate disk or directory with SHA-256
  verification.
- Use Netscape `cookies.txt` and context-qualified PO Token files for streams
  that require them.
- Control a recording service over an authenticated SSH tunnel.
- Delete a completed or stopped job and its local recording folder from the
  desktop app's Library.

## Architecture

- **Recording service:** Rust service that owns jobs, storage, recovery, and
  background workers.
- **Desktop app:** Tauri 2 + React interface for recordings, channels, the
  Library, and settings.
- **CLI:** The same service can be controlled without a GUI, including on a
  Linux server.
- **Media tools:** `yt-dlp` selects streams and FFmpeg captures and validates
  media. Release bundles include the required tools.

## Requirements

- Rust stable
- Node.js 22 or newer
- pnpm 11
- macOS: Xcode Command Line Tools
- Linux: Tauri WebKitGTK 4.1 and the platform development libraries

## Development

```sh
pnpm install
pnpm dev
```

On the first run, open **Settings -> Install verified tools** to install
`yt-dlp`, Deno, FFmpeg, and `ffprobe`. Existing system tools are detected when
available.

Build the desktop frontend with:

```sh
pnpm build
```

Release bundles are written under `target/release/bundle/`. Code signing and
notarization require the distributor's own certificates; no signing keys are
included in this repository.

## CLI and Linux Server

Build the CLI without GUI dependencies:

```sh
cargo build --release -p ytlr
./target/release/ytlr tools install
./target/release/ytlr record 'https://www.youtube.com/watch?v=VIDEO_ID'
./target/release/ytlr record 'https://www.youtube.com/watch?v=VIDEO_ID' --from-now
./target/release/ytlr watch 'https://www.youtube.com/@CHANNEL'
./target/release/ytlr status --json
./target/release/ytlr stop JOB_ID
./target/release/ytlr recover JOB_ID
./target/release/ytlr export JOB_ID --index 0
./target/release/ytlr events JOB_ID
./target/release/ytlr doctor
```

Most commands start the service in the background when needed. Use
`ytlr run --headless` for a foreground process managed by systemd or launchd.
`ytlr shutdown` stops the service cleanly. Jobs that were not explicitly
stopped can resume on the next service start.

Use `--data-dir PATH` or `YTLR_HOME` to run an isolated data and service
instance. The macOS app bundle includes the CLI at
`YTLiveRecord.app/Contents/MacOS/ytlr`.

## Recording Semantics

Recording from the current point is the default. The desktop option
**Save from the beginning when possible** enables yt-dlp's experimental
`--live-from-start` behavior. The platform can only provide past segments that
are still available.

The service keeps separate attempts after reconnects and never silently joins
uncertain boundaries. A user-stopped recording is shown as **Saved** when its
captured files are retained. **Partially saved** is reserved for missing or
unverified media continuity. A service interruption keeps source files and
queues the job for resumption.

The completion check covers container and audio/video headers and media length;
it does not prove that every moment of the public broadcast was available.

## Storage and Recovery

Each job has its own directory:

```text
recordings/VIDEO_ID_JOB_ID/
├── attempt-0001/
│   ├── session.json
│   ├── durable-fragments.jsonl
│   ├── segments.csv / source.*-FragN
│   ├── part-*.mkv / source.*
│   └── result.json
├── attempt-0002/
└── recording.json
```

SQLite uses WAL mode and full synchronization. A job's source fragments,
segments, and finalized results are retained so recovery can be attempted
without overwriting an earlier attempt. This can require two to three times
the size of the final video.

The **Recover source** action creates a playable result from available source
tracks or completed segments. It does not guarantee that unavailable network
media can be reconstructed.

## Backups and Remote Control

The backup destination must already exist. YTLiveRecord identifies the volume,
publishes a finalized file list only after every copy is verified, and repairs
same-size files whose hashes differ. Backup failures do not stop recording.

Remote control uses an SSH tunnel while keeping the server API bound to
localhost. Configure a remote with the CLI:

```sh
ytlr remote add studio \
  --ssh recorder@server \
  --remote-data-dir /home/recorder/ytlr-data \
  --executable /home/recorder/.local/bin/ytlr
ytlr --remote studio status
```

SSH key authentication and trusted host keys must be configured first. A
remote recording can be requested as a second capture target, but the two
results are not automatically merged.

## Testing

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
pnpm check
pnpm test
pnpm test:integration
pnpm test:ui
pnpm test:remote
```

Long-running local HLS tests are available for failure injection and restart
validation:

```sh
pnpm test:soak --seconds 600
pnpm test:soak --seconds 86400
```

The CI workflows cover core and GUI builds on Linux and Windows. Public live
compatibility, 24- to 72-hour operation, power loss, physical disk failure,
and platform-specific signing still require field validation.

## License

The project code is MIT licensed. Bundled external tools have their own
licenses. See [THIRD_PARTY.md](THIRD_PARTY.md) for versions and sources.
