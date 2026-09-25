import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  releaseConfig,
  validateVersion,
  platformForTarget,
  createManifest,
  mergeManifests,
  verifySigningSetup,
  verifyManifestAssets,
} from "../../packaging/updater.mjs";
import { verifyArtifactSignature } from "../../packaging/updater-signature.mjs";

const cli = fileURLToPath(
  new URL(
    "../../apps/desktop/node_modules/@tauri-apps/cli/tauri.js",
    import.meta.url,
  ),
);
const signerEnv = { ...process.env };
for (const name of [
  "TAURI_SIGNING_PRIVATE_KEY",
  "TAURI_SIGNING_PRIVATE_KEY_PATH",
  "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
])
  delete signerEnv[name];
function runSigner(args) {
  // Capture all output, including the generator's private-key text.
  const result = spawnSync(process.execPath, [cli, "signer", ...args], {
    stdio: "pipe",
    env: signerEnv,
  });
  assert.equal(result.status, 0, "Tauri signer command succeeded");
}

test("Tauri-generated public keys and version-bound signatures work with release config", async () => {
  const home = await mkdtemp(join(tmpdir(), "ytlr-signing-test-"));
  try {
    const key = join(home, "test.key");
    runSigner([
      "generate",
      "--ci",
      "--password",
      "test-only-password",
      "--write-keys",
      key,
    ]);
    const publicKey = await readFile(`${key}.pub`, "utf8");
    const config = releaseConfig(publicKey);
    assert.equal(config.plugins.updater.requireSignedVersion, true);
    const artifact = join(home, "test.AppImage");
    await writeFile(artifact, "test artifact bytes");
    runSigner([
      "sign",
      "--private-key-path",
      key,
      "--password",
      "test-only-password",
      "--app-version",
      "1.2.3",
      artifact,
    ]);
    const signature = await readFile(`${artifact}.sig`, "utf8");
    await verifyArtifactSignature(artifact, signature, publicKey, "1.2.3");
    await assert.rejects(
      verifyArtifactSignature(artifact, signature, publicKey, "1.2.4"),
      /version/,
    );
    const editedComment = Buffer.from(
      Buffer.from(signature.trim(), "base64")
        .toString()
        .replace("\tversion:1.2.3", "\tversion:9.9.9"),
    ).toString("base64");
    await assert.rejects(
      verifyArtifactSignature(artifact, editedComment, publicKey, "9.9.9"),
      /verification failed/,
    );
    const env = {
      ...signerEnv,
      TAURI_SIGNING_PRIVATE_KEY: await readFile(key, "utf8"),
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "test-only-password",
    };
    await verifySigningSetup(publicKey, "1.2.3", env);
    await assert.rejects(
      verifySigningSetup(publicKey, "1.2.3", {
        ...env,
        TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "wrong",
      }),
      /preflight failed/,
    );
    const other = join(home, "other.key");
    runSigner(["generate", "--ci", "--password", "", "--write-keys", other]);
    await assert.rejects(
      verifySigningSetup(await readFile(`${other}.pub`, "utf8"), "1.2.3", env),
      /does not match/,
    );
    await writeFile(artifact, "tampered artifact");
    await assert.rejects(
      verifyArtifactSignature(artifact, signature, publicKey, "1.2.3"),
      /verification failed/,
    );
    assert(
      Buffer.from(signature.trim(), "base64")
        .toString("utf8")
        .includes("\tversion:1.2.3"),
    );
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test("release configuration requires a real public-key format and consistent versions", () => {
  const raw = Buffer.concat([Buffer.from("Ed"), Buffer.alloc(40)]);
  const publicKey = Buffer.from(
    `untrusted comment: test public key\n${raw.toString("base64")}\n`,
  ).toString("base64");
  const config = releaseConfig(publicKey, "owner/recorder");
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.equal(config.plugins.updater.pubkey, publicKey);
  assert.equal(
    config.plugins.updater.endpoints[0],
    "https://github.com/owner/recorder/releases/latest/download/latest.json",
  );
  assert.throws(() => releaseConfig(""));
  assert.throws(() => releaseConfig("/path/to/key.pub"));
  assert.throws(() => releaseConfig(publicKey, "owner/repo/extra"));
  assert.throws(() => releaseConfig(`${publicKey}!`));
  assert.equal(validateVersion("v1.2.3", ["1.2.3", "1.2.3"]), "1.2.3");
  assert.throws(() => validateVersion("v1.2.3", ["1.2.2"]));
  assert.throws(() => validateVersion("latest", ["1.2.3"]));
  assert.throws(() => validateVersion("v1.2.3-01", ["1.2.3-01"]));
  assert.throws(() => validateVersion("v1.2.3-beta_test", ["1.2.3-beta_test"]));
  assert.equal(
    validateVersion("v1.2.3-beta.1+build.2", ["1.2.3-beta.1+build.2"]),
    "1.2.3-beta.1+build.2",
  );
  assert.equal(platformForTarget("aarch64-apple-darwin"), "darwin-aarch64");
  assert.throws(() => platformForTarget("unknown"));
});

test("signed artifacts produce installer-specific manifests and a complete release", async () => {
  const home = await mkdtemp(join(tmpdir(), "ytlr-updater-test-"));
  try {
    const key = join(home, "test.key");
    runSigner(["generate", "--ci", "--password", "", "--write-keys", key]);
    const publicKey = await readFile(`${key}.pub`, "utf8");
    const common = {
      repository: "owner/recorder",
      tag: "v1.2.3",
      version: "1.2.3",
      publicKey,
    };
    const manifests = [];
    for (const [target, names] of [
      ["aarch64-apple-darwin", ["YTLR.app.tar.gz"]],
      ["x86_64-pc-windows-msvc", ["YTLR setup.exe", "YTLR.msi"]],
      ["x86_64-unknown-linux-gnu", ["YTLR.AppImage"]],
    ]) {
      const bundleRoot = join(home, target);
      await mkdir(bundleRoot);
      for (const name of names) {
        await writeFile(join(bundleRoot, name), "fixture artifact");
        runSigner([
          "sign",
          "--private-key-path",
          key,
          "--password",
          "",
          "--app-version",
          "1.2.3",
          join(bundleRoot, name),
        ]);
      }
      manifests.push(await createManifest({ ...common, bundleRoot, target }));
    }
    const merged = mergeManifests(manifests);
    assert.equal(
      merged.platforms["windows-x86_64-msi"].signature,
      (
        await readFile(
          join(home, "x86_64-pc-windows-msvc", "YTLR.msi.sig"),
          "utf8",
        )
      ).trim(),
    );
    assert(
      merged.platforms["windows-x86_64-nsis"].url.endsWith("YTLR%20setup.exe"),
    );
    assert.deepEqual(
      merged.platforms["windows-x86_64"],
      merged.platforms["windows-x86_64-nsis"],
    );
    assert(merged.platforms["darwin-aarch64"]);
    assert(merged.platforms["linux-x86_64-appimage"]);
    await verifyManifestAssets(merged, home, common);
    await assert.rejects(
      verifyManifestAssets(merged, home, {
        ...common,
        repository: "wrong/repo",
      }),
      /URL/,
    );
    await assert.rejects(
      verifyManifestAssets(merged, home, { ...common, tag: "v9.9.9" }),
      /tag/,
    );
    const linux = join(home, "x86_64-unknown-linux-gnu", "YTLR.AppImage");
    await writeFile(linux, "corrupted in transit");
    await assert.rejects(
      verifyManifestAssets(merged, home, common),
      /verification failed/,
    );
    await writeFile(linux, "fixture artifact");
    assert.throws(() => mergeManifests(manifests.slice(0, 2)), /Missing linux/);
    assert.throws(
      () => mergeManifests([...manifests, manifests[0]]),
      /Duplicate/,
    );
    assert.throws(
      () =>
        mergeManifests([
          { ...manifests[0], version: "2.0.0" },
          ...manifests.slice(1),
        ]),
      /Mismatched/,
    );
    await rm(join(home, "aarch64-apple-darwin", "YTLR.app.tar.gz.sig"));
    await assert.rejects(
      createManifest({
        ...common,
        bundleRoot: join(home, "aarch64-apple-darwin"),
        target: "aarch64-apple-darwin",
      }),
    );
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});
