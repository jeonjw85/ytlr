// Opt-in network smoke test: node tests/integration/live.mjs URL [--from-start]
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import assert from "node:assert/strict";

const url = process.argv[2];
if (!url?.startsWith("https://"))
  throw new Error("Pass a public live URL explicitly");
const home =
  process.env.YTLR_SMOKE_HOME ?? (await mkdtemp(join(tmpdir(), "ytlr-live-")));
const cli = resolve(process.env.YTLR_SMOKE_BINARY ?? "target/debug/ytlr");
function command(args) {
  const result = spawnSync(cli, ["--data-dir", home, ...args], {
    encoding: "utf8",
    timeout: 300_000,
  });
  if (result.status !== 0) throw new Error(result.stderr);
  return result.stdout;
}
command(["start"]);
const endpoint = JSON.parse(await readFile(join(home, "service.json"), "utf8"));
async function request(path, method = "GET", body) {
  const r = await fetch(`http://127.0.0.1:${endpoint.port}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${endpoint.token}`,
      "x-ytlr-api-version": String(endpoint.api_version),
      "content-type": "application/json",
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const result = await r.json();
  if (!r.ok) throw new Error(result.error);
  return result;
}
const snapshot = await request("/snapshot");
if (snapshot.tools.some((t) => t.error)) command(["tools", "install"]);
const job = await request("/jobs", "POST", {
  url,
  live_from_start: process.argv.includes("--from-start"),
});
let observed;
try {
  const end = Date.now() + 60_000;
  while (Date.now() < end) {
    await new Promise((r) => setTimeout(r, 3000));
    observed = (await request("/snapshot")).jobs.find((j) => j.id === job.id);
    console.log(
      JSON.stringify({
        state: observed.state,
        attempt: observed.attempt,
        bytes: observed.bytes,
        last_media_at: observed.last_media_at,
      }),
    );
    if (observed.state === "failed") throw new Error(observed.message);
  }
  assert(observed.last_media_at, "media progress must be observable");
  assert.equal(
    observed.attempt,
    1,
    "no unexpected restart during smoke capture",
  );
} finally {
  await request(`/jobs/${job.id}/stop`, "POST", {});
}
for (let i = 0; i < 60; i++) {
  await new Promise((r) => setTimeout(r, 1000));
  observed = (await request("/snapshot")).jobs.find((j) => j.id === job.id);
  if (["partial", "stopped", "failed", "completed"].includes(observed.state))
    break;
}
assert(
  observed.outputs.some((o) => o.has_video && o.has_audio && o.duration > 0),
  "playable video + audio after stop",
);
console.log(
  JSON.stringify({ job_id: job.id, outputs: observed.outputs }, null, 2),
);
