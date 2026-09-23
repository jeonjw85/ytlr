// Configurable real-time soak: --seconds 600 (or 86400/259200 for 24/72 hours).
// Two real FFmpeg receivers, rotating HLS, network failure, backup disconnection,
// service SIGKILL/restart, progress and RSS reporting. All media is generated locally.
import { spawn, spawnSync } from "node:child_process";
import {
  mkdtemp,
  mkdir,
  readFile,
  writeFile,
  copyFile,
  chmod,
  readdir,
  rename,
  stat,
  appendFile,
} from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import assert from "node:assert/strict";

const index = process.argv.indexOf("--seconds");
const seconds = index >= 0 ? Number(process.argv[index + 1]) : 600;
assert(
  Number.isFinite(seconds) && seconds >= 120 && seconds <= 259200,
  "duration must be 120..259200 seconds",
);
if (process.platform === "win32")
  throw new Error("Run the real-time soak on macOS or Linux");
const home =
  process.env.YTLR_SOAK_HOME ?? (await mkdtemp(join(tmpdir(), "ytlr-soak-")));
const reportPath = join(home, "report.json");
const root = resolve(".");
const cli = join(home, "ytlr");
await copyFile(
  resolve(process.env.YTLR_SOAK_BINARY ?? "target/debug/ytlr"),
  cli,
);
await chmod(cli, 0o755);
const media = join(home, "hls"),
  data = join(home, "data"),
  backup = join(home, "backup");
await mkdir(media);
await mkdir(data);
await mkdir(backup);
const extractor = join(home, "extractor");
await copyFile(join(root, "tests/fixtures/fake-extractor.mjs"), extractor);
await chmod(extractor, 0o755);
const producer = spawn(
  "ffmpeg",
  [
    "-v",
    "error",
    "-re",
    "-f",
    "lavfi",
    "-i",
    "testsrc2=size=160x90:rate=10",
    "-re",
    "-f",
    "lavfi",
    "-i",
    "sine=frequency=440:sample_rate=48000",
    "-c:v",
    "libx264",
    "-preset",
    "ultrafast",
    "-g",
    "10",
    "-b:v",
    "80k",
    "-c:a",
    "aac",
    "-b:a",
    "32k",
    "-f",
    "hls",
    "-hls_time",
    "2",
    "-hls_list_size",
    "40",
    "-hls_flags",
    "delete_segments+program_date_time+temp_file",
    "-hls_segment_filename",
    join(media, "segment-%09d.ts"),
    join(media, "live.m3u8"),
  ],
  { stdio: ["ignore", "ignore", "pipe"] },
);
producer.stderr.on("data", (x) => process.stderr.write(x));
let outageUntil = 0;
const server = createServer(async (req, res) => {
  if (Date.now() < outageUntil && req.url.startsWith("/abcdefghijk/")) {
    res.writeHead(503);
    res.end();
    return;
  }
  const filename = req.url.split("/").at(-1);
  if (!/^(live\.m3u8|segment-\d+\.ts)$/.test(filename)) {
    res.writeHead(404);
    res.end();
    return;
  }
  try {
    const body = await readFile(join(media, filename));
    res.writeHead(200, {
      "Content-Length": body.length,
      "Content-Type": filename.endsWith("m3u8")
        ? "application/vnd.apple.mpegurl"
        : "video/mp2t",
    });
    res.end(body);
  } catch {
    res.writeHead(503);
    res.end();
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const env = {
  ...process.env,
  YTLR_HOME: data,
  YTLR_YT_DLP: extractor,
  YTLR_DENO: extractor,
  YTLR_FIXTURE: home,
  YTLR_SOAK_SOURCE: `http://127.0.0.1:${server.address().port}`,
  YTLR_FFMPEG:
    process.env.YTLR_FFMPEG ??
    spawnSync("which", ["ffmpeg"], { encoding: "utf8" }).stdout.trim(),
  YTLR_FFPROBE:
    process.env.YTLR_FFPROBE ??
    spawnSync("which", ["ffprobe"], { encoding: "utf8" }).stdout.trim(),
};
let daemon, endpoint;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let cancelled = false;
process.on("SIGINT", () => {
  cancelled = true;
});
process.on("SIGTERM", () => {
  cancelled = true;
});
const report = {
  status: "running",
  started_at: new Date().toISOString(),
  requested_seconds: seconds,
  elapsed_seconds: 0,
  max_rss_mib: 0,
  network_failure_injected: false,
  backup_disconnected: false,
  service_restarted: false,
  samples: 0,
  artifacts: home,
};
async function api(path, method = "GET", body) {
  const r = await fetch(`http://127.0.0.1:${endpoint.port}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${endpoint.token}`,
      "x-ytlr-api-version": String(endpoint.api_version),
      "content-type": "application/json",
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const v = await r.json();
  if (!r.ok) throw new Error(v.error);
  return v;
}
async function until(fn, label, timeout = 90) {
  const end = Date.now() + timeout * 1000;
  let error;
  while (Date.now() < end) {
    try {
      const result = await fn();
      if (result) return result;
    } catch (e) {
      error = e;
    }
    await sleep(500);
  }
  throw new Error(`Timeout: ${label}: ${error ?? ""}`);
}
async function start() {
  daemon = spawn(cli, ["run", "--headless"], {
    env,
    stdio: ["ignore", "ignore", "pipe"],
  });
  daemon.stderr.on("data", (x) => process.stderr.write(x));
  await until(async () => {
    endpoint = JSON.parse(await readFile(join(data, "service.json"), "utf8"));
    return endpoint.pid === daemon.pid && (await api("/health")).ok;
  }, "daemon");
}
console.log(`Soak report: ${reportPath}`);
let detached = false;
try {
  await until(
    async () => (await stat(join(media, "live.m3u8"))).size > 0,
    "HLS production",
  );
  await start();
  let snapshot = await api("/snapshot");
  await api("/settings", "PUT", {
    ...snapshot.settings,
    max_recordings: 2,
    min_free_bytes: 2 * 1024 ** 3,
    stall_timeout_secs: 30,
    backup_root: backup,
  });
  const jobs = [];
  for (const id of ["abcdefghijk", "lmnopqrstuv"])
    jobs.push(
      await api("/jobs", "POST", {
        url: `https://youtu.be/${id}`,
        live_from_start: false,
      }),
    );
  await until(
    async () => (await api("/snapshot")).jobs.every((j) => j.last_media_at),
    "both receivers",
  );
  const begin = Date.now();
  let lastSample = begin;
  let restoreAt = 0;
  let backupFailureObserved = false;
  while (!cancelled && (Date.now() - begin) / 1000 < seconds) {
    const elapsed = (Date.now() - begin) / 1000;
    assert(
      Date.now() - lastSample < 60_000,
      "sampling paused: host sleep or excessive resource contention",
    );
    lastSample = Date.now();
    if (
      !report.network_failure_injected &&
      elapsed > Math.min(60, seconds * 0.2)
    ) {
      outageUntil = Date.now() + 8000;
      report.network_failure_injected = true;
    }
    if (
      !report.backup_disconnected &&
      elapsed > Math.min(100, seconds * 0.35)
    ) {
      await rename(backup, `${backup}.detached`);
      detached = true;
      restoreAt = Date.now() + 15000;
      report.backup_disconnected = true;
    }
    if (detached && Date.now() > restoreAt) {
      await rename(`${backup}.detached`, backup);
      detached = false;
    }
    if (!report.service_restarted && elapsed > Math.min(150, seconds * 0.55)) {
      daemon.kill("SIGKILL");
      await new Promise((r) => daemon.once("exit", r));
      await start();
      report.service_restarted = true;
    }
    snapshot = await api("/snapshot");
    assert(
      snapshot.tools.every((t) => !t.error),
      `engine health: ${JSON.stringify(snapshot.tools)}`,
    );
    assert(
      !snapshot.jobs.some((j) => j.state === "failed"),
      "receiver should recover rather than permanently fail",
    );
    assert(
      snapshot.jobs.every(
        (j) => Date.now() - Date.parse(j.last_media_at) < 180_000,
      ),
      "receiver progress did not recover within 3 minutes",
    );
    assert(
      !snapshot.jobs.some((j) => j.gaps?.some((g) => g.status === "recovered")),
      "unverified receipt boundaries must not be certified",
    );
    backupFailureObserved ||= snapshot.jobs.some(
      (j) => j.backup?.state === "failed",
    );
    const rss =
      Number(
        spawnSync("ps", ["-o", "rss=", "-p", String(daemon.pid)], {
          encoding: "utf8",
        }).stdout.trim(),
      ) / 1024;
    report.max_rss_mib = Math.max(report.max_rss_mib, rss || 0);
    report.samples++;
    report.elapsed_seconds = elapsed;
    await appendFile(
      join(home, "samples.jsonl"),
      JSON.stringify({
        at: new Date().toISOString(),
        rss_mib: rss,
        jobs: snapshot.jobs.map((j) => ({
          id: j.id,
          state: j.state,
          attempt: j.attempt,
          bytes: j.bytes,
          last_media_at: j.last_media_at,
          backup: j.backup?.state,
        })),
      }) + "\n",
    );
    await writeFile(reportPath, JSON.stringify(report, null, 2));
    await sleep(5000);
  }
  report.elapsed_seconds = (Date.now() - begin) / 1000;
  assert(
    Date.now() - lastSample < 60_000,
    "sampling paused before final verification",
  );
  report.status = "verifying";
  await writeFile(reportPath, JSON.stringify(report, null, 2));
  if (cancelled) throw new Error("soak cancelled");
  await until(
    async () =>
      (await api("/snapshot")).jobs.every(
        (j) =>
          j.state === "recording" &&
          Date.now() - Date.parse(j.last_media_at) < 20000,
      ),
    "receivers healthy after faults",
  );
  assert(backupFailureObserved, "backup disconnect must be visible");
  for (const job of jobs) await api(`/jobs/${job.id}/stop`, "POST", {});
  snapshot = await until(
    async () => {
      const s = await api("/snapshot");
      return (
        s.jobs.every((j) =>
          ["partial", "stopped", "completed"].includes(j.state),
        ) && s
      );
    },
    "finalization",
    180,
  );
  for (const job of snapshot.jobs) {
    assert(
      job.outputs.some((o) => o.has_video && o.has_audio && o.duration > 0),
    );
    for (const output of job.outputs) {
      const decode = spawnSync(
        "ffmpeg",
        ["-v", "error", "-xerror", "-i", output.path, "-f", "null", "-"],
        { timeout: 180000 },
      );
      assert.equal(decode.status, 0, decode.stderr.toString());
    }
  }
  await until(
    async () =>
      (await api("/snapshot")).jobs.every((j) => j.backup.state === "verified"),
    "final backups",
    180,
  );
  assert(
    report.max_rss_mib < 512,
    "service RSS budget for the synthetic fixture",
  );
  report.status = "passed";
  report.outputs = snapshot.jobs.map((j) => ({
    id: j.id,
    attempts: j.attempt,
    outputs: j.outputs.length,
    gaps: j.gaps.length,
  }));
} catch (e) {
  report.status = cancelled ? "cancelled" : "failed";
  report.error = String(e);
  process.exitCode = 1;
  console.error(e);
} finally {
  if (detached) await rename(`${backup}.detached`, backup).catch(() => {});
  try {
    await api("/shutdown", "POST");
  } catch {}
  if (daemon && daemon.exitCode === null) {
    await Promise.race([
      new Promise((r) => daemon.once("exit", r)),
      sleep(30000),
    ]);
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
  producer.kill("SIGTERM");
  await new Promise((r) => server.close(r));
  report.finished_at = new Date().toISOString();
  await writeFile(reportPath, JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
}
