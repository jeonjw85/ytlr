// macOS native bundle smoke: no Homebrew/Python/Node on the child's PATH.
import { spawn } from "node:child_process";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import assert from "node:assert/strict";

if (process.platform !== "darwin")
  throw new Error("This native smoke test targets macOS");
const home = await mkdtemp(join(tmpdir(), "ytlr-packaged-"));
const bundle = resolve("target/release/bundle/macos/YTLiveRecord.app");
const app = spawn(join(bundle, "Contents/MacOS/ytlr-desktop"), [], {
  env: {
    ...process.env,
    YTLR_HOME: home,
    PATH: "/usr/bin:/bin:/usr/sbin:/sbin",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
let diagnostics = "";
app.stderr.on("data", (data) => {
  diagnostics += data.toString();
});
let endpoint;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function api(path, method = "GET") {
  const response = await fetch(`http://127.0.0.1:${endpoint.port}${path}`, {
    method,
    headers: { authorization: `Bearer ${endpoint.token}` },
  });
  assert(response.ok);
  return response.json();
}
try {
  for (let i = 0; i < 150; i++) {
    if (app.exitCode !== null)
      throw new Error(`Native app exited: ${diagnostics}`);
    try {
      endpoint = JSON.parse(await readFile(join(home, "service.json"), "utf8"));
      await api("/health");
      break;
    } catch {
      await sleep(300);
    }
  }
  assert(endpoint, `Native UI did not start the service: ${diagnostics}`);
  const snapshot = await api("/snapshot");
  assert.equal(snapshot.tools.length, 4);
  for (const tool of snapshot.tools) {
    assert.equal(tool.error, null, `${tool.name}: ${tool.error}`);
    assert(
      tool.path.startsWith(bundle),
      `${tool.name} must resolve inside the app bundle`,
    );
  }
  app.kill("SIGTERM");
  await sleep(500);
  assert((await api("/health")).ok, "GUI exit must leave the service running");
  console.log(
    "PASS: native macOS GUI startup, actual IPC, all four bundled engines with minimal PATH, service survives GUI exit",
  );
} finally {
  if (app.exitCode === null) app.kill("SIGTERM");
  if (endpoint) {
    try {
      await api("/shutdown", "POST");
    } catch {}
  }
}
