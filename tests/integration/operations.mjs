import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  mkdtemp,
  mkdir,
  readFile,
  writeFile,
  copyFile,
  symlink,
  chmod,
} from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

if (process.platform === "win32") {
  console.log("Process-crash operations tests require macOS/Linux.");
  process.exit(0);
}
const home = await mkdtemp(join(tmpdir(), "ytlr-operations-"));
const cli = join(home, process.platform === "win32" ? "ytlr.exe" : "ytlr");
await copyFile(
  resolve(`target/debug/ytlr${process.platform === "win32" ? ".exe" : ""}`),
  cli,
);
const realFfmpeg = spawnSync("which", ["ffmpeg"], {
  encoding: "utf8",
}).stdout.trim();
const slowFfmpeg = join(home, "ffmpeg-wrapper");
await writeFile(
  slowFfmpeg,
  `#!/usr/bin/env node\nimport {spawn} from 'node:child_process';\nconst args=process.argv.slice(2);\nif(args.includes('-progress') && args.at(-1).includes('preview-')) args.splice(args.indexOf('-i'),0,'-re');\nconst child=spawn(process.env.YTLR_REAL_FFMPEG,args,{stdio:'inherit'});\nchild.on('exit',code=>process.exit(code??1));\n`,
);
await chmod(slowFfmpeg, 0o755);
const fixture = join(home, "source.mp4");
assert.equal(
  spawnSync("ffmpeg", [
    "-v",
    "error",
    "-f",
    "lavfi",
    "-i",
    "testsrc2=size=160x90:rate=10",
    "-f",
    "lavfi",
    "-i",
    "sine=sample_rate=48000",
    "-t",
    "8",
    "-c:v",
    "libx264",
    "-preset",
    "ultrafast",
    "-g",
    "10",
    "-c:a",
    "aac",
    fixture,
  ]).status,
  0,
);
const original = await readFile(fixture);
let daemon, endpoint;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(fn, label, seconds = 60) {
  let last;
  const end = Date.now() + seconds * 1000;
  while (Date.now() < end) {
    try {
      const result = await fn();
      if (result) return result;
    } catch (e) {
      last = e;
    }
    await sleep(100);
  }
  throw new Error(`Timeout: ${label}: ${last ?? ""}`);
}
async function call(path, method = "GET", body) {
  const r = await fetch(`http://127.0.0.1:${endpoint.port}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${endpoint.token}`,
      "x-ytlr-api-version": String(endpoint.api_version),
      "content-type": "application/json",
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const value = await r.json();
  if (!r.ok) throw new Error(value.error);
  return value;
}
async function start() {
  daemon = spawn(cli, ["--data-dir", home, "run", "--headless"], {
    stdio: "ignore",
    env: {
      ...process.env,
      YTLR_FFMPEG: slowFfmpeg,
      YTLR_REAL_FFMPEG: realFfmpeg,
    },
  });
  await until(async () => {
    endpoint = JSON.parse(await readFile(join(home, "service.json"), "utf8"));
    return endpoint.pid === daemon.pid && (await call("/health")).ok;
  }, "service start");
}
try {
  await start();
  const snapshot = await call("/snapshot");
  await call("/settings", "PUT", {
    ...snapshot.settings,
    min_free_bytes: 0,
    prevent_sleep: false,
  });
  const job = await call("/jobs", "POST", {
    url: "https://youtu.be/operation01",
    schedule: { start_at: "2099-01-01T00:00:00Z" },
  });
  await call(`/jobs/${job.id}/stop`, "POST", {});
  await mkdir(job.output_dir, { recursive: true });
  await copyFile(fixture, join(job.output_dir, "export-source.mp4"));
  await writeFile(join(job.output_dir, "cookies.txt"), "PRIVATE CONTENT");
  await assert.rejects(() =>
    call(`/jobs/${job.id}/file-info`, "POST", { path: "cookies.txt" }),
  );
  await assert.rejects(() =>
    call(`/jobs/${job.id}/file-info`, "POST", { path: "../source.mp4" }),
  );
  if (process.platform !== "win32") {
    await symlink(fixture, join(job.output_dir, "export-link.mp4"));
    await assert.rejects(() =>
      call(`/jobs/${job.id}/file-info`, "POST", { path: "export-link.mp4" }),
    );
  }
  await assert.rejects(() =>
    call("/operations", "POST", [
      { job_id: job.id, task: { kind: "preview", path: "export-source.mp4" } },
      { job_id: job.id, task: { kind: "export", index: 99 } },
    ]),
  );
  assert.equal(
    (await call("/operations")).length,
    0,
    "invalid batches must not be partially enqueued",
  );
  const requests = [
    { job_id: job.id, task: { kind: "preview", path: "export-source.mp4" } },
    ...Array.from({ length: 5 }, (_, i) => ({
      job_id: job.id,
      task: {
        kind: "range_clip",
        path: "export-source.mp4",
        start: i * 0.5,
        end: i * 0.5 + 1,
      },
    })),
  ];
  const queued = await call("/operations", "POST", requests);
  assert(queued.some((o) => o.state === "queued"));
  await until(async () => {
    const op = (await call("/operations")).find((o) => o.id === queued[0].id);
    if (op.state === "failed") throw new Error(op.error);
    return (
      op.state === "running" &&
      op.message === "미디어 처리 중" &&
      op.progress !== null
    );
  }, "active preview before forced restart");
  daemon.kill("SIGKILL");
  await new Promise((r) => daemon.once("exit", r));
  if (process.platform !== "win32") {
    const pattern =
      home.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "/workers/render-";
    await until(
      async () => spawnSync("pgrep", ["-f", pattern]).status === 1,
      "render supervisor exits on parent death",
      35,
    );
  }
  await start();
  await until(async () => {
    const ops = await call("/operations");
    const failed = ops.find((o) => o.state === "failed");
    if (failed) throw new Error(failed.error);
    return queued.every(
      (q) => ops.find((o) => o.id === q.id)?.state === "completed",
    );
  }, "persistent queue replay");
  const operations = await call("/operations");
  const preview = operations.find((o) => o.id === queued[0].id);
  const name = preview.result.path.split(/[\\/]/).pop();
  const proxy = await readFile(preview.result.path);
  assert.equal(
    createHash("sha256").update(proxy).digest("hex"),
    preview.result.output.sha256,
  );
  assert.deepEqual(
    await readFile(join(job.output_dir, "export-source.mp4")),
    original,
  );
  const probe = JSON.parse(
    spawnSync(
      "ffprobe",
      ["-v", "error", "-show_streams", "-of", "json", preview.result.path],
      { encoding: "utf8" },
    ).stdout,
  );
  assert(probe.streams.some((s) => s.codec_name === "h264"));
  assert(probe.streams.some((s) => s.codec_name === "aac"));
  const ticket = await call(`/jobs/${job.id}/preview-ticket`, "POST", {
    path: name,
  });
  const again = await call(`/jobs/${job.id}/preview-ticket`, "POST", {
    path: name,
  });
  assert.equal(again.ticket, ticket.ticket, "reuse scoped preview tickets");
  const url = `http://127.0.0.1:${endpoint.port}/play/${ticket.ticket}`;
  const range = await fetch(url, {
    headers: { Range: "bytes=0-127", Origin: "http://tauri.localhost" },
  });
  assert.equal(range.status, 206);
  assert.equal(
    range.headers.get("content-range"),
    `bytes 0-127/${proxy.length}`,
  );
  assert.deepEqual(
    Buffer.from(await range.arrayBuffer()),
    proxy.subarray(0, 128),
  );
  assert.equal(
    (await fetch(url, { headers: { Range: "bytes=999999999999-" } })).status,
    416,
  );
  assert.equal(
    (await fetch(`http://127.0.0.1:${endpoint.port}/play/unknown`)).status,
    404,
  );
  const retry = await call("/operations", "POST", [
    { job_id: job.id, task: { kind: "preview", path: "export-missing.mp4" } },
    {
      job_id: job.id,
      task: { kind: "range_clip", path: "export-source.mp4", start: 6, end: 7 },
    },
  ]);
  await call(`/operations/${retry[1].id}/cancel`, "POST", {});
  await until(
    async () =>
      (await call("/operations")).find((o) => o.id === retry[0].id)?.state ===
      "failed",
    "failure status",
  );
  await copyFile(fixture, join(job.output_dir, "export-missing.mp4"));
  await call(`/operations/${retry[0].id}/retry`, "POST", {});
  await until(
    async () =>
      (await call("/operations")).find((o) => o.id === retry[0].id)?.state ===
      "completed",
    "explicit retry",
  );
  assert.equal(
    (await call("/operations")).find((o) => o.id === retry[1].id).state,
    "cancelled",
  );
  const bundle = await call("/configuration/export");
  assert(!JSON.stringify(bundle).includes("cookies_path"));
  const imported = {
    bundle,
    replace_channels: false,
    apply_settings: false,
    storage_root: null,
  };
  await call("/configuration/preview", "POST", imported);
  await call("/configuration/import", "POST", imported);
  console.log(
    "PASS: durable queue restart, batch atomicity, cancellation/retry, H.264/AAC previews, byte-range playback, source preservation and file access boundaries",
  );
} finally {
  try {
    await call("/shutdown", "POST", {});
  } catch {}
  if (daemon?.exitCode === null) {
    await Promise.race([
      new Promise((r) => daemon.once("exit", r)),
      sleep(15000),
    ]);
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
}
