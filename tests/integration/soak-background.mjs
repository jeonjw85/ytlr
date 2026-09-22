// Launch a self-contained 24h soak; this shell does not need to remain open.
import { spawn } from "node:child_process";
import { mkdtemp, open, copyFile, chmod, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
const home = await mkdtemp(join(tmpdir(), "ytlr-soak-24h-"));
const tools = resolve(
  "target/release/bundle/macos/YTLiveRecord.app/Contents/Resources/tools",
);
for (const name of ["ffmpeg", "ffprobe"]) {
  await copyFile(join(tools, name), join(home, name));
  await chmod(join(home, name), 0o755);
}
const log = await open(join(home, "run.log"), "a", 0o600);
const child = spawn(
  process.execPath,
  [resolve("tests/integration/soak.mjs"), "--seconds", "86400"],
  {
    cwd: resolve("."),
    detached: true,
    stdio: ["ignore", log.fd, log.fd],
    env: {
      ...process.env,
      YTLR_SOAK_HOME: home,
      YTLR_SOAK_BINARY: resolve("target/release/ytlr"),
      YTLR_FFMPEG: join(home, "ffmpeg"),
      YTLR_FFPROBE: join(home, "ffprobe"),
    },
  },
);
child.unref();
await log.close();
const started = {
  pid: child.pid,
  report: join(home, "report.json"),
  log: join(home, "run.log"),
  stop: `kill -TERM ${child.pid}`,
  status: "started_not_verified",
};
await writeFile(join(home, "launcher.json"), JSON.stringify(started, null, 2));
console.log(JSON.stringify(started, null, 2));
