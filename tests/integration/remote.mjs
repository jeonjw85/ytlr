// Real OpenSSH loopback integration. No system sshd configuration is modified.
import { spawn, spawnSync } from "node:child_process";
import {
  mkdtemp,
  mkdir,
  readFile,
  writeFile,
  chmod,
  copyFile,
  stat,
  open,
} from "node:fs/promises";
import { createHash } from "node:crypto";
import { createServer } from "node:net";
import { tmpdir, userInfo } from "node:os";
import { join, resolve } from "node:path";
import assert from "node:assert/strict";

if (process.platform === "win32")
  throw new Error("Use macOS/Linux with an OpenSSH server for this test");
const home = await mkdtemp(join(tmpdir(), "ytlr-ssh-"));
const cli = join(home, "ytlr");
await copyFile(resolve("target/debug/ytlr"), cli);
const extractor = join(home, "extractor");
await copyFile(resolve("tests/fixtures/fake-extractor.mjs"), extractor);
await chmod(extractor, 0o755);
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
    "3",
    "-c:v",
    "libx264",
    "-preset",
    "ultrafast",
    "-c:a",
    "aac",
    join(home, "sample.mkv"),
  ]).status,
  0,
);
const a = join(home, "source"),
  b = join(home, "remote with ' quote");
await mkdir(a);
await mkdir(b);
const host = join(home, "host"),
  identity = join(home, "identity"),
  known = join(home, "known_hosts");
for (const key of [host, identity]) {
  const generated = spawnSync("ssh-keygen", [
    "-q",
    "-t",
    "ed25519",
    "-N",
    "",
    "-f",
    key,
  ]);
  assert.equal(generated.status, 0, generated.stderr.toString());
}
const probe = createServer();
await new Promise((r) => probe.listen(0, "127.0.0.1", r));
const port = probe.address().port;
await new Promise((r) => probe.close(r));
await writeFile(
  known,
  `[127.0.0.1]:${port} ${(await readFile(`${host}.pub`, "utf8")).trim()}\n`,
);
await chmod(known, 0o600);
const config = join(home, "sshd_config");
await writeFile(
  config,
  `Port ${port}\nListenAddress 127.0.0.1\nHostKey ${host}\nPidFile ${home}/sshd.pid\nAuthorizedKeysFile ${identity}.pub\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM ${process.platform === "linux" ? "yes" : "no"}\nStrictModes ${process.platform === "linux" ? "no" : "yes"}\nAllowUsers ${userInfo().username}\nLogLevel ERROR\n`,
);
let sshd;
const processes = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(fn, label, seconds = 45) {
  const end = Date.now() + seconds * 1000;
  let error;
  while (Date.now() < end) {
    try {
      const value = await fn();
      if (value) return value;
    } catch (e) {
      error = e;
    }
    await sleep(200);
  }
  throw new Error(`Timeout: ${label}: ${error ?? ""}`);
}
async function command(data, args) {
  const p = spawn(cli, ["--data-dir", data, ...args]);
  let out = "",
    err = "";
  p.stdout.on("data", (x) => (out += x));
  p.stderr.on("data", (x) => (err += x));
  const code = await new Promise((r) => p.once("exit", r));
  if (code !== 0) throw new Error(err);
  return JSON.parse(out || "null");
}
async function api(data, path, method = "GET", body, extra = {}) {
  const ep = JSON.parse(await readFile(join(data, "service.json"), "utf8"));
  const r = await fetch(`http://127.0.0.1:${ep.port}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${ep.token}`,
      "x-ytlr-api-version": String(ep.api_version),
      "content-type": "application/json",
      ...extra,
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const v = await r.json();
  if (!r.ok) throw new Error(v.error);
  return v;
}
async function start(data) {
  const p = spawn(cli, ["--data-dir", data, "run", "--headless"], {
    stdio: "ignore",
    env: {
      ...process.env,
      YTLR_YT_DLP: extractor,
      YTLR_DENO: extractor,
      YTLR_FIXTURE: home,
    },
  });
  processes.push(p);
  await until(async () => {
    const ep = JSON.parse(await readFile(join(data, "service.json"), "utf8"));
    return ep.pid === p.pid && (await api(data, "/health")).ok;
  }, "service");
  return p;
}
function sshStart() {
  let error = "";
  sshd = spawn("/usr/sbin/sshd", ["-D", "-e", "-f", config], {
    stdio: ["ignore", "ignore", "pipe"],
  });
  sshd.stderr.on("data", (x) => (error += x));
  return () => error;
}
try {
  const errors = sshStart();
  await sleep(500);
  assert.equal(sshd.exitCode, null, errors());
  // This checks actual authentication and host-key validation before testing YTLiveRecord.
  const ssh = spawnSync(
    "ssh",
    [
      "-o",
      "BatchMode=yes",
      "-o",
      "StrictHostKeyChecking=yes",
      "-o",
      `UserKnownHostsFile=${known}`,
      "-i",
      identity,
      "-p",
      String(port),
      `${userInfo().username}@127.0.0.1`,
      "true",
    ],
    { timeout: 15000, encoding: "utf8" },
  );
  if (ssh.status !== 0)
    throw new Error(
      `Local sshd authentication unavailable: ${ssh.stderr} ${errors()}`,
    );
  let primary = await start(a);
  await start(b);
  await command(a, [
    "remote",
    "add",
    "replica",
    "--ssh",
    `${userInfo().username}@127.0.0.1:${port}`,
    "--remote-data-dir",
    b,
    "--identity",
    identity,
    "--known-hosts",
    known,
    "--executable",
    cli,
  ]);
  await command(b, [
    "remote",
    "add",
    "back",
    "--ssh",
    `${userInfo().username}@127.0.0.1:${port}`,
    "--remote-data-dir",
    a,
    "--identity",
    identity,
    "--known-hosts",
    known,
    "--executable",
    cli,
  ]);
  const selected = await command(a, [
    "--remote",
    "replica",
    "status",
    "--json",
  ]);
  assert.equal(selected.jobs.length, 0);
  for (const [data, target] of [
    [a, "replica"],
    [b, "back"],
  ]) {
    const snap = await api(data, "/snapshot");
    await api(data, "/settings", "PUT", {
      ...snap.settings,
      min_free_bytes: 9_000_000_000_000,
      replica_remote: target,
    });
  }
  const request = {
    url: "https://youtu.be/abcdefghijk",
    live_from_start: true,
    recording_options: { audio_only: false, max_height: 720 },
    stop_at: new Date(Date.now() + 3600000).toISOString(),
    schedule: {
      start_at: new Date(Date.now() + 1800000).toISOString(),
      duration_minutes: 120,
    },
  };
  const initial = await api(a, "/jobs", "POST", request);
  await Promise.all(
    Array.from({ length: 12 }, () => api(a, "/jobs", "POST", request)),
  );
  const delivered = await until(async () => {
    const job = (await api(a, "/snapshot")).jobs[0];
    return job.replica?.state === "accepted" && job;
  }, "replica request accepted");
  assert.equal(
    delivered.replica.remote_state,
    "queued",
    "request receipt must not be reported as recording",
  );
  const target = (await api(b, "/snapshot")).jobs;
  assert.equal(target.length, 1);
  assert.equal(target[0].replica_origin, true);
  assert.equal(target[0].replica, null, "no forwarding loop");
  assert.deepEqual(target[0].recording_options, request.recording_options);
  assert.equal(Date.parse(target[0].stop_at), Date.parse(request.stop_at));
  assert.equal(
    Date.parse(target[0].schedule.start_at),
    Date.parse(request.schedule.start_at),
  );
  assert.equal(target[0].schedule.duration_minutes, 120);
  const changedStop = new Date(Date.now() + 7200000).toISOString();
  await api(a, `/jobs/${initial.id}/schedule`, "PUT", { stop_at: changedStop });
  await until(
    async () =>
      Date.parse((await api(b, "/snapshot")).jobs[0].stop_at) ===
      Date.parse(changedStop),
    "updated replica deadline",
  );
  await api(a, `/jobs/${initial.id}/start-schedule`, "PUT", {
    start_at: null,
    duration_minutes: 120,
  });
  await until(
    async () => (await api(b, "/snapshot")).jobs[0].schedule.start_at === null,
    "updated replica start schedule",
  );
  for (const data of [a, b]) {
    const snap = await api(data, "/snapshot");
    await api(data, "/settings", "PUT", {
      ...snap.settings,
      min_free_bytes: 0,
    });
  }
  await until(
    async () => {
      const job = (await api(a, "/snapshot")).jobs[0];
      return (
        job.state === "recording" &&
        job.last_media_at &&
        job.replica.state === "recording"
      );
    },
    "actual source and replica media receipt",
    60,
  );
  await api(b, `/jobs/${target[0].id}/stop`, "POST", {});
  const replay = await api(b, "/jobs", "POST", request, {
    "x-ytlr-fanout": "1",
    "idempotency-key": delivered.replica.request_id,
  });
  assert.equal(
    replay.id,
    target[0].id,
    "replay after completion is still idempotent",
  );
  const current = await api(a, "/snapshot");
  await api(a, "/settings", "PUT", {
    ...current.settings,
    min_free_bytes: 9_000_000_000_000,
  });
  primary.kill("SIGKILL");
  await new Promise((r) => primary.once("exit", r));
  primary = await start(a);
  assert.equal(
    (await api(a, "/snapshot")).jobs[0].replica.request_id,
    delivered.replica.request_id,
    "outbox survives restart",
  );
  const failed = spawn(cli, [
    "--data-dir",
    a,
    "--remote",
    "replica",
    "run",
    "--headless",
  ]);
  failed.stderr.resume();
  failed.stdout.resume();
  assert.notEqual(
    await new Promise((r) => failed.once("exit", r)),
    0,
    "local-only command rejected before execution",
  );
  await api(a, `/jobs/${initial.id}/stop`, "POST", {});
  await until(
    async () =>
      (await api(a, "/snapshot")).jobs.find((j) => j.id === initial.id)
        .state === "stopped",
    "source stopped before transfer",
  );
  const remoteJob = await until(async () => {
    const j = (await api(b, "/snapshot")).jobs.find(
      (j) => j.id === target[0].id,
    );
    return j.state === "stopped" && j;
  }, "remote finalized");
  const transfer = Buffer.alloc(128 * 1024 * 1024, 0x35);
  await writeFile(join(remoteJob.output_dir, "export-transfer.mp4"), transfer);
  const transferHash = createHash("sha256").update(transfer).digest("hex");
  const localSettings = (await api(a, "/snapshot")).settings;
  await api(a, "/settings", "PUT", { ...localSettings, min_free_bytes: 0 });
  const [download] = await api(a, "/operations", "POST", [
    {
      job_id: remoteJob.id,
      task: {
        kind: "download",
        remote: "replica",
        path: "export-transfer.mp4",
      },
    },
  ]);
  const receiving = await until(async () => {
    const op = (await api(a, "/operations")).find((o) => o.id === download.id);
    if (op.state === "failed") throw new Error(op.error);
    return op.bytes_done > 0 && op.bytes_done < transfer.length && op;
  }, "partial SSH download");
  await api(a, `/operations/${download.id}/cancel`, "POST", {});
  const cancelled = await until(async () => {
    const op = (await api(a, "/operations")).find((o) => o.id === download.id);
    return op.state === "cancelled" && op;
  }, "download cancellation");
  const partial = cancelled.destination.replace(/\.[^.]+$/, ".partial");
  const received = (await stat(partial)).size;
  assert(received >= receiving.bytes_done && received < transfer.length);
  primary.kill("SIGKILL");
  await new Promise((r) => primary.once("exit", r));
  primary = await start(a);
  assert.equal(
    (await api(a, "/operations")).find((o) => o.id === download.id).state,
    "cancelled",
  );
  transfer[0] ^= 1;
  await writeFile(join(remoteJob.output_dir, "export-transfer.mp4"), transfer);
  await api(a, `/operations/${download.id}/retry`, "POST", {});
  const changed = await until(async () => {
    const op = (await api(a, "/operations")).find((o) => o.id === download.id);
    return op.state === "failed" && op;
  }, "changed remote source rejected");
  assert(changed.error.includes("원격 파일이 변경"));
  assert.equal((await stat(partial)).size, received);
  transfer[0] ^= 1;
  await writeFile(join(remoteJob.output_dir, "export-transfer.mp4"), transfer);
  const damaged = await open(partial, "r+");
  await damaged.write(Buffer.from([0xff]), 0, 1, 0);
  await damaged.close();
  await api(a, `/operations/${download.id}/retry`, "POST", {});
  const corrupt = await until(
    async () => {
      const op = (await api(a, "/operations")).find(
        (o) => o.id === download.id,
      );
      return op.state === "failed" && op;
    },
    "corrupt partial download rejected",
    90,
  );
  assert(corrupt.error.includes("해시 불일치"));
  assert.equal((await stat(partial)).size, 0);
  await api(a, `/operations/${download.id}/retry`, "POST", {});
  const downloaded = await until(
    async () => {
      const op = (await api(a, "/operations")).find(
        (o) => o.id === download.id,
      );
      if (op.state === "failed") throw new Error(op.error);
      return op.state === "completed" && op;
    },
    "resumed SSH download",
    90,
  );
  assert.equal(downloaded.result.sha256, transferHash);
  assert.equal(
    createHash("sha256")
      .update(await readFile(downloaded.result.path))
      .digest("hex"),
    transferHash,
  );
  assert.equal(downloaded.attempts, 4);
  await assert.rejects(() =>
    api(b, `/jobs/${remoteJob.id}/file-info`, "POST", {
      path: "../service.json",
    }),
  );
  console.log(
    "[remote] persistent SSH download, cancellation, resume and SHA-256 verification passed",
  );
  console.log(
    "PASS: real SSH authentication, quoted paths, tunnel API health, CLI target selection, idempotent replica replay, loop prevention, durable delivery state, accurate accepted-vs-recording status",
  );
  console.log(`SSH test artifacts: ${home}`);
} finally {
  for (const data of [a, b]) {
    try {
      await api(data, "/shutdown", "POST");
    } catch {}
  }
  await sleep(500);
  for (const p of processes) if (p.exitCode === null) p.kill("SIGTERM");
  sshd?.kill("SIGTERM");
}
