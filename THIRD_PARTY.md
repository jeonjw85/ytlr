# External tools

The MIT license at the repository root covers YTLiveRecord's own code. External
executables retain their own licenses. They run as separate processes.

| Tool | Pinned release | Upstream / source | License |
| --- | --- | --- | --- |
| yt-dlp | 2026.08.19 | https://github.com/yt-dlp/yt-dlp/tree/2026.08.19 | Source: Unlicense; standalone distributions include GPLv3+ components; see upstream THIRD_PARTY_LICENSES.txt |
| Deno | v2.9.7 | https://github.com/denoland/deno/tree/v2.9.7 | MIT and bundled third-party notices |
| FFmpeg / ffprobe (macOS) | Official source 9.0.2 | https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz | LGPL-2.1-or-later, default native codecs and Apple system frameworks; no `--enable-nonfree` |
| FFmpeg / ffprobe (Linux) | BtbN 8.1.3, autobuild-2026-09-21-13-55 | https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-09-21-13-55 | GPL build; source/build recipes in BtbN/FFmpeg-Builds |
| FFmpeg / ffprobe (Windows) | ffmpeg-static b6.1.1 | https://github.com/eugeneware/ffmpeg-static/releases/tag/b6.1.1 | GPL build; platform README and license accompany binaries |

All binary assets are pinned by SHA-256 in `crates/engine/src/tools.rs`; filenames
or release labels alone are not trusted. FFmpeg builds differ across platforms;
`ytlr doctor` reports the actual binary version.

macOS uses `packaging/build-ffmpeg.mjs`: source SHA-256 is pinned independently,
nonfree flags are rejected, and `otool` verifies that only system dynamic libraries
are referenced. The application includes the corresponding source archive, build
recipe, configuration and binary checksums under `Contents/Resources/tools/sources`
and `FFmpeg-build-README.txt`. Earlier third-party macOS binaries contained nonfree
components and are no longer downloaded or packaged. The macOS standalone CLI needs
a local FFmpeg installation or the app's bundled tools for `tools install`.

FFmpeg binary build recipes, patches, source-version references and upstream
build-provider links are recorded in the platform README assets in the
ffmpeg-static release. These must accompany redistributions alongside the
applicable license texts and corresponding-source availability. The packaging
script copies notices into the application resources.

Relevant source projects:
- https://github.com/FFmpeg/FFmpeg
- https://github.com/eugeneware/ffmpeg-static
- https://github.com/yt-dlp/yt-dlp/blob/2026.08.19/THIRD_PARTY_LICENSES.txt
- https://github.com/denoland/deno/blob/v2.9.7/LICENSE.md

Rust and JavaScript dependencies are locked in Cargo.lock and pnpm-lock.yaml.
Tauri, React, and other dependencies retain their upstream license notices.
