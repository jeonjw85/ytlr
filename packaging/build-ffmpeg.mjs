// Build an independently replaceable, system-framework-only macOS FFmpeg.
// No --enable-nonfree binaries are redistributed.
import { spawnSync } from "node:child_process";
import { mkdir, readFile, writeFile, access } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { join, resolve } from "node:path";
const version = "9.0.2";
const sourceHash =
  "8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e";
export async function buildFfmpeg(root) {
  const cache = join(root, "packaging/.ffmpeg-source");
  await mkdir(cache, { recursive: true });
  const archive = join(cache, `ffmpeg-${version}.tar.xz`),
    source = join(cache, `ffmpeg-${version}`);
  try {
    await access(archive);
  } catch {
    const response = await fetch(
      `https://ffmpeg.org/releases/ffmpeg-${version}.tar.xz`,
    );
    if (!response.ok) throw new Error("FFmpeg source download failed");
    await writeFile(archive, Buffer.from(await response.arrayBuffer()));
  }
  const digest = createHash("sha256")
    .update(await readFile(archive))
    .digest("hex");
  if (digest !== sourceHash) throw new Error("FFmpeg source checksum mismatch");
  const run = (command, args, cwd) => {
    const r = spawnSync(command, args, {
      cwd,
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
      env: { ...process.env, MACOSX_DEPLOYMENT_TARGET: "11.0" },
    });
    if (r.status !== 0)
      throw new Error(
        `${command} failed: ${r.stderr?.slice(-12000)} ${r.stdout?.slice(-12000)}`,
      );
    return r.stdout;
  };
  try {
    await access(join(source, "configure"));
  } catch {
    run("tar", ["-xf", archive, "-C", cache], root);
  }
  const flags = [
    "--disable-autodetect",
    "--disable-doc",
    "--disable-debug",
    "--disable-ffplay",
    "--disable-shared",
    "--enable-static",
    "--disable-x86asm",
    "--enable-securetransport",
    "--enable-videotoolbox",
    "--enable-audiotoolbox",
    "--enable-zlib",
  ];
  try {
    await access(join(source, "ytlr-built.json"));
  } catch {
    console.log(`Building FFmpeg ${version} from verified source…`);
    run("./configure", flags, source);
    run("make", ["-j8", "ffmpeg", "ffprobe"], source);
    const manifest = {
      version,
      source_sha256: sourceHash,
      flags,
      binaries: {},
    };
    for (const name of ["ffmpeg", "ffprobe"]) {
      const binary = join(source, name);
      const info = run(binary, ["-version"], source);
      if (info.includes("--enable-nonfree"))
        throw new Error("Nonfree build rejected");
      const libraries = run("otool", ["-L", binary], source)
        .split("\n")
        .slice(1)
        .map((s) => s.trim().split(" ")[0])
        .filter(Boolean);
      if (
        libraries.some(
          (p) =>
            !p.startsWith("/usr/lib/") && !p.startsWith("/System/Library/"),
        )
      )
        throw new Error(`Non-system dynamic dependency: ${libraries}`);
      manifest.binaries[name] = createHash("sha256")
        .update(await readFile(binary))
        .digest("hex");
    }
    await writeFile(
      join(source, "ytlr-built.json"),
      JSON.stringify(manifest, null, 2),
    );
  }
  return { source, archive };
}
if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
  await buildFfmpeg(resolve("."));
