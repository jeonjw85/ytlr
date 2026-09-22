import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  mkdtemp,
  mkdir,
  readFile,
  writeFile,
  copyFile,
  chmod,
  rename,
  readdir,
  stat,
} from "node:fs/promises";
import { createServer } from "node:http";
import { createReadStream } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

if (process.platform === "win32") {
  console.log("POSIX process-crash integration suite: use macOS/Linux.");
  process.exit(0);
}
const root = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const home = await mkdtemp(join(tmpdir(), "ytlr-integration-"));
const cli = join(root, "target/debug/ytlr");
const fixture = join(home, "extractor");
await copyFile(join(root, "tests/fixtures/fake-extractor.mjs"), fixture);
await chmod(fixture, 0o755);
const generate = spawnSync(
  "ffmpeg",
  [
    "-v",
    "error",
    "-f",
    "lavfi",
    "-i",
    "testsrc2=size=160x90:rate=10",
    "-f",
    "lavfi",
    "-i",
    "sine=frequency=440:sample_rate=48000",
    "-t",
    "65",
    "-c:v",
    "libx264",
    "-preset",
    "ultrafast",
    "-g",
    "10",
    "-c:a",
    "aac",
    "-y",
    join(home, "sample.mkv"),
  ],
  { stdio: "inherit" },
);
assert.equal(generate.status, 0, "FFmpeg fixture generation");
const convert = spawnSync("ffmpeg", [
  "-v",
  "error",
  "-i",
  join(home, "sample.mkv"),
  "-c",
  "copy",
  "-movflags",
  "+faststart",
  join(home, "sample.mp4"),
]);
assert.equal(convert.status, 0);
const source = await readFile(join(home, "sample.mp4"));
const server = createServer((req, res) => {
  const match = /bytes=(\d+)-(\d*)/.exec(req.headers.range ?? "");
  const start = match ? Number(match[1]) : 0,
    end = match?.[2] ? Number(match[2]) : source.length - 1;
  res.writeHead(match ? 206 : 200, {
    "Content-Type": "video/mp4",
    "Accept-Ranges": "bytes",
    "Content-Length": end - start + 1,
    ...(match
      ? { "Content-Range": `bytes ${start}-${end}/${source.length}` }
      : {}),
  });
  res.end(source.subarray(start, end + 1));
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const env = {
  ...process.env,
  YTLR_HOME: home,
  YTLR_YT_DLP: fixture,
  YTLR_DENO: fixture,
  YTLR_FIXTURE: home,
  YTLR_FFMPEG: spawnSync("which", ["ffmpeg"], {
    encoding: "utf8",
  }).stdout.trim(),
  YTLR_FFPROBE: spawnSync("which", ["ffprobe"], {
    encoding: "utf8",
  }).stdout.trim(),
  YTLR_FIXTURE_MEDIA: `http://127.0.0.1:${server.address().port}/sample.mp4`,
};
let daemon;
let endpoint;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(callback, label, seconds = 40) {
  const deadline = Date.now() + seconds * 1000;
  let last;
  while (Date.now() < deadline) {
    try {
      const result = await callback();
      if (result) return result;
    } catch (e) {
      last = e;
    }
    await sleep(150);
  }
  throw new Error(`Timeout: ${label}${last ? ` (${last})` : ""}`);
}
async function request(path, method = "GET", body) {
  const r = await fetch(`http://127.0.0.1:${endpoint.port}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${endpoint.token}`,
      "content-type": "application/json",
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const json = await r.json();
  if (!r.ok) throw new Error(json.error);
  return json;
}
async function start() {
  daemon = spawn(cli, ["run", "--headless"], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  daemon.stderr.on("data", (chunk) => process.stderr.write(chunk));
  await until(async () => {
    endpoint = JSON.parse(await readFile(join(home, "service.json"), "utf8"));
    if (endpoint.pid !== daemon.pid) return false;
    return (await request("/health")).ok;
  }, "service start");
}
try {
  await start();
  const unauthorized = await fetch(
    `http://127.0.0.1:${endpoint.port}/snapshot`,
  );
  assert.equal(unauthorized.status, 401);
  const badOrigin = await fetch(`http://127.0.0.1:${endpoint.port}/snapshot`, {
    headers: {
      authorization: `Bearer ${endpoint.token}`,
      origin: "https://example.test",
    },
  });
  assert.equal(badOrigin.status, 401);
  let snap = await request("/snapshot");
  assert(
    snap.tools.every((t) => !t.error),
    JSON.stringify(snap.tools),
  );
  const backupRoot = join(home, "backup");
  await mkdir(backupRoot, { recursive: true });
  const cookieFile = join(home, "user-cookies.txt"),
    tokenFile = join(home, "user-token.txt");
  const cookieText =
    "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tTEST_COOKIE_SECRET\n";
  await writeFile(cookieFile, cookieText);
  await writeFile(tokenFile, "mweb.gvs+TEST_PO_TOKEN_abcdefghijklmnop\n");
  await writeFile(join(home, "require-auth"), "1");
  await request("/settings", "PUT", {
    ...snap.settings,
    max_recordings: 1,
    min_free_bytes: 0,
    backup_root: backupRoot,
    cookies_path: cookieFile,
    po_token_path: tokenFile,
  });
  const first = await request("/jobs", "POST", {
    url: "https://youtu.be/abcdefghijk",
  });
  const duplicate = await request("/jobs", "POST", {
    url: "https://youtube.com/live/abcdefghijk",
  });
  assert.equal(first.id, duplicate.id, "duplicate URL canonicalization");
  const second = await request("/jobs", "POST", {
    url: "https://youtu.be/lmnopqrstuv",
  });
  await until(async () => {
    const s = await request("/snapshot");
    return s.jobs.find((j) => j.id === first.id)?.last_media_at;
  }, "native progress");
  snap = await request("/snapshot");
  assert.equal(
    snap.jobs.find((j) => j.id === second.id).state,
    "queued",
    "concurrency limit",
  );
  await request(`/jobs/${second.id}/stop`, "POST", {});
  assert.equal(
    (await request("/snapshot")).jobs.find((j) => j.id === second.id).state,
    "stopped",
  );
  await until(async () => {
    const j = (await request("/snapshot")).jobs.find((j) => j.id === first.id);
    return j.bytes > 0;
  }, "durable data");
  const ledgerPath = join(
    first.output_dir,
    "attempt-0001",
    "durable-fragments.jsonl",
  );
  const ledger = await until(async () => {
    const x = await readFile(ledgerPath, "utf8");
    return x.includes("sha256") && x;
  }, "durable ledger");
  const committed = JSON.parse(ledger.split("\n")[0]);
  const committedPath = join(first.output_dir, "attempt-0001", committed.file);
  const before = await readFile(committedPath);
  const backupFragment = join(
    backupRoot,
    first.video_id,
    first.id,
    "attempt-0001",
    committed.file,
  );
  await until(
    async () => (await stat(backupFragment)).size === before.length,
    "committed backup",
  );
  await writeFile(backupFragment, Buffer.alloc(before.length, 0));
  await until(
    async () => (await readFile(backupFragment)).equals(before),
    "same-size backup corruption repaired",
  );
  await rename(backupRoot, `${backupRoot}.detached`);
  await until(
    async () =>
      (await request("/snapshot")).jobs.find((j) => j.id === first.id).backup
        .state === "failed",
    "backup disconnect reported",
  );
  assert.equal(
    (await request("/snapshot")).jobs.find((j) => j.id === first.id).state,
    "recording",
    "backup failure does not interrupt capture",
  );
  await rename(`${backupRoot}.detached`, backupRoot);
  await until(
    async () =>
      (await request("/snapshot")).jobs.find((j) => j.id === first.id).backup
        .state === "in_progress",
    "backup reconnect retries",
  );
  const events = await request(`/jobs/${first.id}/events`);
  assert(!JSON.stringify(events).includes("TEST_COOKIE_SECRET"));
  assert(!JSON.stringify(events).includes("TEST_PO_TOKEN_abcdefghijklmnop"));
  assert.equal(
    await readFile(cookieFile, "utf8"),
    cookieText,
    "extractor never modifies original cookie jar",
  );
  assert(
    (await readFile(join(home, "auth-checked"), "utf8")).includes("checked"),
    "extractor actually consumed auth config",
  );
  daemon.kill("SIGKILL");
  await until(
    async () => !(await readdir(home)).some((n) => n.startsWith("active-")),
    "orphan downloader cleanup",
  );
  assert.deepEqual(
    await readFile(committedPath),
    before,
    "committed data survives SIGKILL",
  );
  await writeFile(join(home, "version-fail-once"), "1");
  await start();
  await until(async () => {
    const j = (await request("/snapshot")).jobs.find((j) => j.id === first.id);
    return j.attempt >= 2;
  }, "restart recovery");
  const recovered = (await request("/snapshot")).jobs.find(
    (j) => j.id === first.id,
  );
  assert.equal(
    recovered.continuity_uncertain,
    true,
    "unknown restart boundary is explicit",
  );
  assert(
    recovered.attempts.length >= 2,
    "each capture attempt is recorded on the timeline",
  );
  assert(
    recovered.gaps.some((g) => g.after_attempt === 1 && g.seconds >= 0),
    "restart must record the missing wall-clock range",
  );
  await request(`/jobs/${first.id}/stop`, "POST", {});
  await until(async () => {
    const j = (await request("/snapshot")).jobs.find((j) => j.id === first.id);
    return ["partial", "stopped", "failed"].includes(j.state);
  }, "stopped job finalization");
  await request(`/jobs/${first.id}/recover`, "POST", {});
  await until(
    async () => {
      const j = (await request("/snapshot")).jobs.find(
        (j) => j.id === first.id,
      );
      return j.state === "partial" && j.outputs.length > 0;
    },
    "source recovery",
    90,
  );
  assert.deepEqual(
    await readFile(committedPath),
    before,
    "recovery never overwrites committed data",
  );

  const ff = await request("/jobs", "POST", {
    url: "https://youtu.be/ffffgggghhh",
    live_from_start: false,
  });
  const finished = await until(async () => {
    const j = (await request("/snapshot")).jobs.find((j) => j.id === ff.id);
    return j.state === "completed" && j;
  }, "real FFmpeg segmentation and merge");
  assert(finished.outputs[0].has_audio && finished.outputs[0].has_video);
  assert(
    finished.outputs[0].duration >= 64 && finished.outputs[0].duration <= 67,
    "expected media duration",
  );
  const segments = await readdir(join(ff.output_dir, "attempt-0001"));
  assert(
    segments.filter((n) => n.startsWith("part-") && n.endsWith(".mkv"))
      .length >= 3,
    "30 second segment rotation",
  );
  const backupJob = join(backupRoot, ff.video_id, ff.id);
  await until(async () => {
    const listing = await readdir(backupJob).catch(() => []);
    return (
      listing.includes("recording.json") &&
      listing.some((n) => n.startsWith("attempt-"))
    );
  }, "secondary disk backup");
  await until(
    async () =>
      (await request("/snapshot")).jobs.find((j) => j.id === ff.id).backup
        .state === "verified",
    "backup process exits while parent lifetime pipe remains open",
  );
  const exported = await request(`/jobs/${ff.id}/export`, "POST", { index: 0 });
  await until(async () => {
    const st = await stat(exported.path);
    return st.size > 0;
  }, "MP4 export");
  const channel = await request("/channels", "POST", {
    url: "https://youtube.com/@fixture",
    name: "Fixture",
  });
  await until(async () => {
    const s = await request("/snapshot");
    return s.jobs.some(
      (j) => j.video_id === "qqqqwwwweee" && j.state === "waiting",
    );
  }, "channel detection and scheduled waiting");
  await request(`/channels/${channel.id}`, "DELETE");
  const waiting = (await request("/snapshot")).jobs.find(
    (j) => j.video_id === "qqqqwwwweee",
  );
  await request(`/jobs/${waiting.id}/stop`, "POST", {});
  console.log(
    "PASS: authenticated IPC, deduplication, concurrency, stop, SIGKILL recovery, orphan cleanup, media segmentation/merge/export, channel monitoring, scheduled waiting",
  );
  console.log(`Integration artifacts: ${home}`);
} finally {
  try {
    await request("/shutdown", "POST", {});
  } catch {}
  if (daemon && daemon.exitCode === null) {
    await Promise.race([
      new Promise((resolve) => daemon.once("exit", resolve)),
      sleep(30000),
    ]);
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
  await new Promise((resolve) => server.close(resolve));
}
