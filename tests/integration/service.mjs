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
  unlink,
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
const cli = join(home, "ytlr");
await copyFile(join(root, "target/debug/ytlr"), cli);
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
const mediaSource = await readFile(join(home, "sample.mp4"));
assert.equal(
  spawnSync("ffmpeg", [
    "-v",
    "error",
    "-i",
    join(home, "sample.mkv"),
    "-vn",
    "-c:a",
    "copy",
    join(home, "sample.mka"),
  ]).status,
  0,
);
const audioSource = await readFile(join(home, "sample.mka"));
const notifications = [];
let rejectNotification = true;
const server = createServer((req, res) => {
  if (req.url === "/notify") {
    let body = "";
    req.on("data", (chunk) => {
      body += chunk;
    });
    req.on("end", () => {
      notifications.push(JSON.parse(body));
      res.writeHead(rejectNotification ? 503 : 200);
      rejectNotification = false;
      res.end("{}");
    });
    return;
  }
  const source = req.url === "/sample.mka" ? audioSource : mediaSource;
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
  YTLR_FIXTURE_AUDIO: `http://127.0.0.1:${server.address().port}/sample.mka`,
  YTLR_TEST_WEBHOOK_URL: `http://127.0.0.1:${server.address().port}/notify`,
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
    signal: AbortSignal.timeout(20000),
    method,
    headers: {
      authorization: `Bearer ${endpoint.token}`,
      "x-ytlr-api-version": String(endpoint.api_version),
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
  console.log("[integration] service/authentication and recording lifecycle");
  const unauthorized = await fetch(
    `http://127.0.0.1:${endpoint.port}/snapshot`,
  );
  assert.equal(unauthorized.status, 401);
  const badOrigin = await fetch(`http://127.0.0.1:${endpoint.port}/snapshot`, {
    headers: {
      authorization: `Bearer ${endpoint.token}`,
      "x-ytlr-api-version": String(endpoint.api_version),
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
    automation: {
      notifications: [
        {
          id: "integration",
          kind: "webhook",
          url_env: "YTLR_TEST_WEBHOOK_URL",
        },
      ],
      retention: {},
    },
  });
  const scheduled = await request("/jobs", "POST", {
    url: "https://youtu.be/starttime01",
    schedule: {
      start_at: new Date(Date.now() + 3600000).toISOString(),
      duration_minutes: 5,
    },
  });
  await sleep(2500);
  assert.equal(
    (await request("/snapshot")).jobs.find((j) => j.id === scheduled.id)
      .attempt,
    0,
    "start schedule must hold queue without occupying a slot",
  );
  await request(`/jobs/${scheduled.id}/start-schedule`, "PUT", {
    start_at: new Date(Date.now() - 1000).toISOString(),
    duration_minutes: 5,
  });
  await until(async () => {
    const j = (await request("/snapshot")).jobs.find(
      (j) => j.id === scheduled.id,
    );
    return (
      j.last_media_at &&
      j.stop_at &&
      Date.parse(j.stop_at) - Date.parse(j.started_at) >= 299000
    );
  }, "scheduled start and duration deadline");
  await request(`/jobs/${scheduled.id}/stop`, "POST", {});
  await until(
    async () =>
      (await request("/snapshot")).jobs.find((j) => j.id === scheduled.id)
        .state === "stopped",
    "scheduled fixture stopped",
  );
  const first = await request("/jobs", "POST", {
    url: "https://youtu.be/abcdefghijk",
    live_from_start: true,
    recording_options: { audio_only: false, max_height: 480 },
    stop_at: new Date(Date.now() + 3600000).toISOString(),
  });
  const duplicate = await request("/jobs", "POST", {
    url: "https://youtube.com/live/abcdefghijk",
    live_from_start: first.live_from_start,
    recording_options: first.recording_options,
    stop_at: first.stop_at,
    priority: first.priority,
  });
  assert.equal(first.id, duplicate.id, "duplicate URL canonicalization");
  const second = await request("/jobs", "POST", {
    url: "https://youtu.be/lmnopqrstuv",
  });
  await until(async () => {
    const s = await request("/snapshot");
    return s.jobs.find((j) => j.id === first.id)?.last_media_at;
  }, "native progress");
  await assert.rejects(
    () => request("/shutdown-idle", "POST", {}),
    /녹화 또는 파일 처리/,
  );
  assert.equal(
    (await request("/health")).ok,
    true,
    "an update must not stop an active recorder",
  );
  const marked = await request(`/jobs/${first.id}/bookmarks`, "POST", {
    title: "First marker",
    note: "Before restart",
  });
  assert.equal(marked.bookmarks[0].attempt, 1);
  assert.equal(
    marked.bookmarks[0].media_seconds,
    null,
    "native wall-clock marker must not invent a playback offset",
  );
  const bookmark = marked.bookmarks[0];
  await request(`/jobs/${second.id}/schedule`, "PUT", {
    stop_at: new Date(Date.now() + 3600000).toISOString(),
  });
  const cancelled = await request(`/jobs/${second.id}/schedule`, "PUT", {
    stop_at: null,
  });
  assert.equal(cancelled.stop_at, null);
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
    90,
  );
  const events = await request(`/jobs/${first.id}/events`);
  await until(
    async () =>
      (await request("/notifications")).some(
        (d) => d.delivered && d.attempts >= 2,
      ),
    "webhook persistent retry succeeds after HTTP 503",
  );
  assert(notifications.length >= 2);
  assert(
    !JSON.stringify(await request("/notifications")).includes(
      env.YTLR_TEST_WEBHOOK_URL,
    ),
    "notification diagnostics omit endpoint credentials",
  );
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
  const overdue = await request("/jobs", "POST", {
    url: "https://youtu.be/deadline001",
    stop_at: new Date(Date.now() + 1000).toISOString(),
  });
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
  await sleep(1500);
  await start();
  await until(async () => {
    const job = (await request("/snapshot")).jobs.find(
      (j) => j.id === overdue.id,
    );
    assert.equal(
      job.attempt,
      0,
      "expired queued job must never start after a restart",
    );
    return job.state === "stopped";
  }, "deadline expired while service was down");
  await until(async () => {
    const j = (await request("/snapshot")).jobs.find((j) => j.id === first.id);
    return j.attempt >= 2;
  }, "restart recovery");
  const recovered = (await request("/snapshot")).jobs.find(
    (j) => j.id === first.id,
  );
  assert.equal(recovered.stop_at, first.stop_at);
  assert.equal(recovered.bookmarks[0].id, bookmark.id);
  assert.equal(recovered.bookmarks[0].note, "Before restart");
  assert.deepEqual(recovered.recording_options, {
    audio_only: false,
    max_height: 480,
  });
  const selectors = (await readFile(join(home, "abcdefghijk.formats"), "utf8"))
    .trim()
    .split("\n");
  assert(selectors.includes("capture:bv*[height<=480]+ba/b[height<=480]"));
  assert(
    selectors
      .filter((s) => s.startsWith("inspect:"))
      .every((s) => s === "inspect:bv*[height<=480]+ba/b[height<=480]"),
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
  await request(`/jobs/${first.id}/schedule`, "PUT", {
    stop_at: new Date(Date.now() - 1000).toISOString(),
  });
  await until(async () => {
    const j = (await request("/snapshot")).jobs.find((j) => j.id === first.id);
    return ["partial", "stopped", "failed"].includes(j.state);
  }, "stopped job finalization");
  assert.equal(
    (await request(`/jobs/${first.id}/events`)).filter(
      (e) => e.kind === "scheduled_stop",
    ).length,
    1,
  );
  await request(`/jobs/${first.id}/bookmarks/${bookmark.id}`, "PUT", {
    title: "Edited marker",
    note: "After restart",
  });
  const metadata = JSON.parse(
    await readFile(join(first.output_dir, "recording.json"), "utf8"),
  );
  assert.equal(metadata.bookmarks[0].title, "Edited marker");
  assert.equal(metadata.bookmarks[0].created_at, bookmark.created_at);
  console.log("[integration] restart deadline and bookmark persistence passed");
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
  await until(
    async () => {
      const listing = await readdir(backupJob).catch(() => []);
      if (
        listing.includes("recording.json") &&
        listing.some((n) => n.startsWith("attempt-"))
      )
        return true;
      const j = (await request("/snapshot")).jobs.find((j) => j.id === ff.id);
      throw new Error(`${j?.backup?.state}: ${j?.backup?.message}`);
    },
    "secondary disk backup",
    120,
  );
  await until(
    async () =>
      (await request("/snapshot")).jobs.find((j) => j.id === ff.id).backup
        .state === "verified",
    "backup process exits while parent lifetime pipe remains open",
    90,
  );
  const exported = await request(`/jobs/${ff.id}/export`, "POST", { index: 0 });
  await until(async () => {
    const st = await stat(exported.path);
    return st.size > 0;
  }, "MP4 export");
  const outputBeforeCleanup = await readFile(finished.outputs[0].path);
  const exportBeforeCleanup = await readFile(exported.path);
  const inventory = await request(`/jobs/${ff.id}/storage`);
  assert(
    inventory.results > 0 && inventory.segments > 0 && inventory.exports > 0,
  );
  const preview = await request(`/jobs/${ff.id}/cleanup/preview`, "POST", {});
  assert(preview.files.length >= 3);
  await assert.rejects(() =>
    request(`/jobs/${ff.id}/cleanup`, "POST", {
      plan_id: "outdated",
      all: true,
    }),
  );
  const cleaned = await request(`/jobs/${ff.id}/cleanup`, "POST", {
    plan_id: preview.id,
    files: [preview.files[0].path],
  });
  assert.equal(cleaned.reclaimed_bytes, preview.files[0].bytes);
  assert.equal(cleaned.completed, true);
  assert.deepEqual(cleaned.pending_files, []);
  assert.deepEqual(
    await readFile(finished.outputs[0].path),
    outputBeforeCleanup,
  );
  assert.deepEqual(await readFile(exported.path), exportBeforeCleanup);
  console.log(
    "[integration] video segmentation/export/selective cleanup passed",
  );
  await assert.rejects(() =>
    request(`/jobs/${ff.id}/cleanup`, "POST", {
      plan_id: preview.id,
      all: true,
    }),
  );
  await assert.rejects(
    () => request(`/jobs/${first.id}/cleanup/preview`, "POST", {}),
    "uncertain continuity must prevent cleanup",
  );
  const audio = await request("/jobs", "POST", {
    url: "https://youtu.be/audioonly01",
    live_from_start: false,
    recording_options: { audio_only: true, max_height: null },
  });
  const audioFinished = await until(async () => {
    const job = (await request("/snapshot")).jobs.find(
      (j) => j.id === audio.id,
    );
    return job.state === "completed" && job;
  }, "audio-only segmentation and finalization");
  assert(audioFinished.outputs.length > 0);
  assert(audioFinished.outputs.every((o) => o.has_audio && !o.has_video));
  assert(audioFinished.outputs[0].duration >= 64);
  const audioExport = await request(`/jobs/${audio.id}/export`, "POST", {
    index: 0,
  });
  assert(audioExport.path.endsWith(".mka"));
  await until(
    async () => (await stat(audioExport.path)).size > 0,
    "audio export",
  );
  const audioProbe = JSON.parse(
    spawnSync(
      "ffprobe",
      ["-v", "error", "-show_streams", "-of", "json", audioExport.path],
      { encoding: "utf8" },
    ).stdout,
  );
  assert.deepEqual(
    audioProbe.streams.map((s) => s.codec_type),
    ["audio"],
  );
  // Native (from-start) source recovery must also accept audio without video.
  const nativeAudio = await request("/jobs", "POST", {
    url: "https://youtu.be/audionative",
    live_from_start: true,
    recording_options: { audio_only: true, max_height: null },
  });
  await until(async () => {
    const job = (await request("/snapshot")).jobs.find(
      (j) => j.id === nativeAudio.id,
    );
    return job.last_media_at && job.bytes > 0;
  }, "native audio data");
  await request(`/jobs/${nativeAudio.id}/schedule`, "PUT", {
    stop_at: new Date(Date.now() - 1000).toISOString(),
  });
  await until(async () => {
    const job = (await request("/snapshot")).jobs.find(
      (j) => j.id === nativeAudio.id,
    );
    return (
      job.state === "stopped" &&
      job.outputs.some((o) => o.has_audio && !o.has_video)
    );
  }, "native audio source salvage");
  console.log("[integration] audio-only segmentation/export/recovery passed");
  const channel = await request("/channels", "POST", {
    url: "https://youtube.com/@fixture",
    name: "Fixture",
    recording_options: { audio_only: true, max_height: null },
    live_from_start: true,
  });
  await until(async () => {
    const s = await request("/snapshot");
    return s.jobs.some(
      (j) => j.video_id === "qqqqwwwweee" && j.state === "waiting",
    );
  }, "channel detection and scheduled waiting");
  assert(
    (await request("/snapshot")).channels.find((c) => c.id === channel.id)
      .health.last_success_at,
  );
  const channelSettings = (await request("/snapshot")).settings;
  await request("/settings", "PUT", {
    ...channelSettings,
    scan_interval_secs: 15,
  });
  await writeFile(join(home, "qqqqwwwweee.fail"), "1");
  await until(
    async () =>
      (await request("/snapshot")).channels.find((c) => c.id === channel.id)
        .health.incident_open,
    "channel failure incident",
    65,
  );
  assert.equal(
    (await request(`/channels/${channel.id}/events`)).filter(
      (e) => e.kind === "channel_failed",
    ).length,
    1,
  );
  await unlink(join(home, "qqqqwwwweee.fail"));
  await until(
    async () =>
      !(await request("/snapshot")).channels.find((c) => c.id === channel.id)
        .health.incident_open,
    "channel recovery incident",
    35,
  );
  assert.equal(
    (await request(`/channels/${channel.id}/events`)).filter(
      (e) => e.kind === "channel_recovered",
    ).length,
    1,
  );
  const testDelivery = await request("/notifications/test", "POST", {
    target_id: "integration",
  });
  await until(
    async () =>
      (await request("/notifications")).find(
        (d) => d.id === testDelivery.delivery_id,
      )?.delivered,
    "notification connection test",
  );
  await request(`/channels/${channel.id}`, "DELETE");
  const waiting = (await request("/snapshot")).jobs.find(
    (j) => j.video_id === "qqqqwwwweee",
  );
  assert.deepEqual(waiting.recording_options, channel.recording_options);
  assert.equal(waiting.live_from_start, true);
  await request(`/jobs/${waiting.id}/stop`, "POST", {});
  snap = await request("/snapshot");
  await request("/settings", "PUT", { ...snap.settings, max_retries: 0 });
  await writeFile(join(home, "alertfail01.fail"), "1");
  const failed = await request("/jobs", "POST", {
    url: "https://youtu.be/alertfail01",
    live_from_start: true,
  });
  await until(
    async () =>
      (await request("/snapshot")).jobs
        .find((j) => j.id === failed.id)
        .alerts.some((a) => a.kind === "recording_failed" && a.active),
    "persistent failure alert",
  );
  await sleep(5500);
  assert.equal(
    (await request(`/jobs/${failed.id}/events`)).filter(
      (e) => e.kind === "alert_opened",
    ).length,
    1,
    "same incident is not repeatedly notified",
  );
  await unlink(join(home, "alertfail01.fail"));
  await request(`/jobs/${failed.id}/retry`, "POST", {});
  await until(
    async () =>
      (await request("/snapshot")).jobs
        .find((j) => j.id === failed.id)
        .alerts.some(
          (a) => a.kind === "recording_failed" && !a.active && a.resolved_at,
        ),
    "recovery notification after media resumes",
  );
  await request(`/jobs/${failed.id}/stop`, "POST", {});
  await until(
    async () =>
      (await request("/snapshot")).jobs.find((j) => j.id === failed.id)
        .state === "stopped",
    "alert fixture stopped",
  );
  console.log(
    "[integration] channel defaults, storage, and alert lifecycle passed",
  );
  snap = await request("/snapshot");
  await request("/settings", "PUT", {
    ...snap.settings,
    min_free_bytes: snap.free_bytes + 1024,
  });
  const low = await until(
    async () =>
      (await request("/snapshot")).storage.find((disk) => disk.low_space),
    "low disk early warning",
  );
  assert(low.free_bytes > 0 && low.total_bytes >= low.free_bytes);
  const updateWaiting = await request("/jobs", "POST", {
    url: "https://youtu.be/updatewait1",
    schedule: { start_at: new Date(Date.now() + 3600000).toISOString() },
  });
  await until(
    () => request("/shutdown-idle", "POST", {}),
    "idle shutdown for app update",
  );
  await until(
    async () => daemon.exitCode !== null,
    "old sidecar fully exits before update",
  );
  await start();
  const resumed = (await request("/snapshot")).jobs.find(
    (j) => j.id === updateWaiting.id,
  );
  assert.equal(resumed.attempt, 0);
  assert.equal(resumed.state, "queued");
  assert.equal(resumed.schedule.start_at, updateWaiting.schedule.start_at);
  await request(`/jobs/${updateWaiting.id}/stop`, "POST", {});
  console.log(
    "PASS: authenticated IPC, deduplication, concurrency, stop timers and restart expiry, bookmarks, SIGKILL recovery, orphan cleanup, video/audio segmentation and export, verified selective cleanup, alert deduplication/recovery, channel defaults and scheduled waiting",
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
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
}
