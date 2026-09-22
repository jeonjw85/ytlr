import { spawnSync } from "node:child_process";
import { mkdir, copyFile, writeFile, access } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { buildFfmpeg } from "./build-ffmpeg.mjs";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const debug = process.argv.includes("--debug");
const run = (command, args, cwd = root) => {
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
    shell: process.platform === "win32" && command === "pnpm",
  });
  if (result.status !== 0) process.exit(result.status ?? 1);
};
const target = spawnSync("rustc", ["--print", "host-tuple"], {
  encoding: "utf8",
}).stdout.trim();
if (!target) throw new Error("Rust target detection failed");
run("cargo", ["build", "-p", "ytlr", ...(debug ? [] : ["--release"])]);
const binary = join(
  root,
  "target",
  debug ? "debug" : "release",
  `ytlr${process.platform === "win32" ? ".exe" : ""}`,
);
const tauri = join(root, "apps/desktop/src-tauri");
await mkdir(join(tauri, "binaries"), { recursive: true });
await mkdir(join(tauri, "resources/tools"), { recursive: true });
await copyFile(
  binary,
  join(
    tauri,
    "binaries",
    `ytlr-${target}${process.platform === "win32" ? ".exe" : ""}`,
  ),
);
try {
  await access(join(tauri, "icons/icon.png"));
} catch {
  run(
    "pnpm",
    [
      "tauri",
      "icon",
      "../../packaging/icon.svg",
      "--output",
      "src-tauri/icons",
    ],
    join(root, "apps/desktop"),
  );
}

if (!debug) {
  let builtFfmpeg;
  if (process.platform === "darwin") {
    builtFfmpeg = await buildFfmpeg(root);
    process.env.YTLR_FFMPEG = join(builtFfmpeg.source, "ffmpeg");
    process.env.YTLR_FFPROBE = join(builtFfmpeg.source, "ffprobe");
  }
  run(binary, [
    "--data-dir",
    join(root, "packaging/.engine-cache"),
    "tools",
    "bundle",
    join(tauri, "resources/tools"),
  ]);
  await copyFile(
    join(root, "THIRD_PARTY.md"),
    join(tauri, "resources/tools/THIRD_PARTY.md"),
  );
  // License texts accompany the bundled executables, not merely a link in the app UI.
  const licenses = {
    "yt-dlp-LICENSE.txt":
      "https://raw.githubusercontent.com/yt-dlp/yt-dlp/2026.08.19/LICENSE",
    "yt-dlp-THIRD_PARTY_LICENSES.txt":
      "https://raw.githubusercontent.com/yt-dlp/yt-dlp/2026.08.19/THIRD_PARTY_LICENSES.txt",
    "deno-LICENSE.txt":
      "https://raw.githubusercontent.com/denoland/deno/v2.9.7/LICENSE.md",
    "FFmpeg-GPL.txt":
      "https://raw.githubusercontent.com/FFmpeg/FFmpeg/n6.0/COPYING.GPLv3",
  };
  const platform = { darwin: "darwin", linux: "linux", win32: "win32" }[
    process.platform
  ];
  const arch = process.arch === "arm64" ? "arm64" : "x64";
  if (builtFfmpeg) {
    const sources = join(tauri, "resources/tools/sources");
    await mkdir(sources, { recursive: true });
    await copyFile(builtFfmpeg.archive, join(sources, "ffmpeg-9.0.2.tar.xz"));
    await copyFile(
      join(root, "packaging/build-ffmpeg.mjs"),
      join(sources, "build-ffmpeg.mjs"),
    );
    await copyFile(
      join(builtFfmpeg.source, "ytlr-built.json"),
      join(tauri, "resources/tools/FFmpeg-build-README.txt"),
    );
    await copyFile(
      join(builtFfmpeg.source, "COPYING.LGPLv2.1"),
      join(tauri, "resources/tools/FFmpeg-build-LICENSE.txt"),
    );
  } else if (process.platform === "linux") {
    await writeFile(
      join(tauri, "resources/tools/FFmpeg-build-README.txt"),
      "FFmpeg 8.1.3 GPL build\nProvider: BtbN/FFmpeg-Builds\nRelease: autobuild-2026-09-21-13-55\nSource/build recipes: https://github.com/BtbN/FFmpeg-Builds\nFFmpeg source: https://github.com/FFmpeg/FFmpeg/tree/n8.1.3\nChecksums: crates/engine/src/tools.rs\n",
    );
    licenses["FFmpeg-build-LICENSE.txt"] =
      "https://raw.githubusercontent.com/FFmpeg/FFmpeg/n8.1/COPYING.GPLv3";
  } else {
    licenses["FFmpeg-build-README.txt"] =
      `https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/${platform}-${arch}.README`;
    licenses["FFmpeg-build-LICENSE.txt"] =
      `https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/${platform}-${arch}.LICENSE`;
  }
  for (const [filename, url] of Object.entries(licenses)) {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`License download failed: ${url}`);
    await writeFile(
      join(tauri, "resources/tools", filename),
      await response.text(),
    );
  }
}
console.log(
  `Desktop assets ready (${target}, ${debug ? "development" : "release"}).`,
);
