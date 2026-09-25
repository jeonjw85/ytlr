import { spawnSync } from "node:child_process";
import {
  readFile,
  writeFile,
  readdir,
  mkdir,
  mkdtemp,
  rm,
} from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import {
  publicKeyBytes,
  verifyArtifactSignature,
} from "./updater-signature.mjs";

const root = dirname(dirname(fileURLToPath(import.meta.url)));

export function releaseConfig(publicKey, repository = "jeonjw85/ytlr") {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository))
    throw new Error("Invalid release repository");
  const key = publicKey?.trim();
  publicKeyBytes(key);
  return {
    bundle: { createUpdaterArtifacts: true },
    plugins: {
      updater: {
        pubkey: key,
        requireSignedVersion: true,
        endpoints: [
          `https://github.com/${repository}/releases/latest/download/latest.json`,
        ],
        windows: { installMode: "passive" },
      },
    },
  };
}

export function validateVersion(tag, versions) {
  const version = tag?.replace(/^v/, "");
  const number = "(?:0|[1-9]\\d*)";
  const prerelease = `(?:${number}|[0-9]*[A-Za-z-][0-9A-Za-z-]*)`;
  const semver = new RegExp(
    `^v${number}\\.${number}\\.${number}(?:-${prerelease}(?:\\.${prerelease})*)?(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?$`,
  );
  if (!semver.test(tag ?? "") || versions.some((v) => v !== version)) {
    throw new Error(
      "Release tag must match workspace, root package, desktop package, and Tauri versions",
    );
  }
  return version;
}

async function projectVersion(tag) {
  const files = [
    "package.json",
    "apps/desktop/package.json",
    "apps/desktop/src-tauri/tauri.conf.json",
  ];
  const versions = await Promise.all(
    files.map(
      async (file) =>
        JSON.parse(await readFile(join(root, file), "utf8")).version,
    ),
  );
  const cargo = await readFile(join(root, "Cargo.toml"), "utf8");
  versions.push(
    cargo.match(/\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1],
  );
  return validateVersion(tag, versions);
}

async function filesUnder(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...(await filesUnder(path)));
    else if (entry.isFile()) files.push(path);
  }
  return files;
}

export function platformForTarget(target) {
  const arch = target.startsWith("aarch64-")
    ? "aarch64"
    : target.startsWith("x86_64-")
      ? "x86_64"
      : null;
  const os = target.includes("apple-darwin")
    ? "darwin"
    : target.includes("windows")
      ? "windows"
      : target.includes("linux")
        ? "linux"
        : null;
  if (!arch || !os) throw new Error(`Unsupported updater target: ${target}`);
  return `${os}-${arch}`;
}

export async function createManifest({
  bundleRoot,
  target,
  repository,
  tag,
  version,
  notes = `YTLR ${version}`,
  publicKey,
}) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository))
    throw new Error("Invalid release repository");
  validateVersion(tag, [version]);
  const platform = platformForTarget(target);
  const files = await filesUnder(bundleRoot);
  const formats = platform.startsWith("darwin-")
    ? [[".app.tar.gz", ""]]
    : platform.startsWith("windows-")
      ? [
          [".exe", "-nsis"],
          [".msi", "-msi"],
        ]
      : [[".AppImage", "-appimage"]];
  const platforms = {};
  for (const [extension, suffix] of formats) {
    const matches = files.filter((file) => file.endsWith(extension));
    if (matches.length !== 1)
      throw new Error(
        `Expected one ${extension} updater artifact for ${platform}, found ${matches.length}`,
      );
    const artifact = matches[0];
    const signature = (await readFile(`${artifact}.sig`, "utf8")).trim();
    if (!signature)
      throw new Error(`Empty updater signature for ${basename(artifact)}`);
    await verifyArtifactSignature(artifact, signature, publicKey, version);
    const entry = {
      signature,
      url: `https://github.com/${repository}/releases/download/${tag}/${encodeURIComponent(basename(artifact))}`,
    };
    platforms[`${platform}${suffix}`] = entry;
    // Older clients use the unqualified target; prefer NSIS on Windows.
    if (!platforms[platform]) platforms[platform] = entry;
  }
  return { version, notes, pub_date: new Date().toISOString(), platforms };
}

export function mergeManifests(manifests) {
  if (!manifests.length) throw new Error("No updater manifests found");
  const first = manifests[0];
  const platforms = {};
  for (const manifest of manifests) {
    if (
      manifest.version !== first.version ||
      !Number.isFinite(Date.parse(manifest.pub_date))
    )
      throw new Error("Mismatched or invalid updater manifests");
    for (const [platform, entry] of Object.entries(manifest.platforms)) {
      if (platforms[platform])
        throw new Error(`Duplicate updater platform: ${platform}`);
      if (!entry.signature || !entry.url.startsWith("https://github.com/"))
        throw new Error(`Invalid updater artifact: ${platform}`);
      platforms[platform] = entry;
    }
  }
  for (const os of ["darwin", "windows", "linux"]) {
    if (!Object.keys(platforms).some((key) => key.startsWith(`${os}-`)))
      throw new Error(`Missing ${os} updater artifact`);
  }
  return {
    version: first.version,
    notes: first.notes,
    pub_date: first.pub_date,
    platforms,
  };
}

function run(command, args, cwd = root) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${basename(command)} failed (${result.status})`);
}

export async function verifySigningSetup(
  publicKey,
  version,
  env = process.env,
) {
  publicKeyBytes(publicKey);
  if (!env.TAURI_SIGNING_PRIVATE_KEY?.trim())
    throw new Error("TAURI_SIGNING_PRIVATE_KEY is required for release builds");
  const temp = await mkdtemp(join(tmpdir(), "ytlr-signing-probe-"));
  try {
    const path = join(temp, "probe.bin");
    await writeFile(path, "YTLR updater signing preflight");
    const result = spawnSync(
      process.execPath,
      [
        join(root, "apps/desktop/node_modules/@tauri-apps/cli/tauri.js"),
        "signer",
        "sign",
        "--app-version",
        version,
        path,
      ],
      {
        env: {
          ...env,
          CI: "true",
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
            env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? "",
        },
        stdio: "pipe",
        timeout: 30000,
      },
    );
    // Never relay signer stdout/stderr: malformed secret input can appear there.
    if (result.error || result.status !== 0)
      throw new Error(
        "Updater signing preflight failed; check private key and password (signer output withheld)",
      );
    await verifyArtifactSignature(
      path,
      await readFile(`${path}.sig`, "utf8"),
      publicKey,
      version,
    );
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}

export async function verifyManifestAssets(
  manifest,
  assets,
  { repository, tag, publicKey },
) {
  validateVersion(tag, [manifest.version]);
  const files = await filesUnder(assets);
  for (const entry of Object.values(manifest.platforms)) {
    const url = new URL(entry.url);
    const name = decodeURIComponent(url.pathname.split("/").at(-1));
    if (
      !name ||
      name.includes("/") ||
      name.includes("\\") ||
      entry.url !==
        `https://github.com/${repository}/releases/download/${tag}/${encodeURIComponent(name)}`
    ) {
      throw new Error(
        "Updater artifact URL does not match the release repository/tag",
      );
    }
    const matches = files.filter((file) => basename(file) === name);
    if (
      matches.length !== 1 ||
      (await readFile(`${matches[0]}.sig`, "utf8")).trim() !== entry.signature
    ) {
      throw new Error(
        `Missing, ambiguous, or mismatched signed asset: ${name}`,
      );
    }
    await verifyArtifactSignature(
      matches[0],
      entry.signature,
      publicKey,
      manifest.version,
    );
  }
}

async function main() {
  const command = process.argv[2];
  const repository = process.env.GITHUB_REPOSITORY ?? "jeonjw85/ytlr";
  const tag = process.env.YTLR_RELEASE_TAG ?? process.env.GITHUB_REF_NAME;
  const version = await projectVersion(tag);
  if (command === "build") {
    const config = releaseConfig(
      process.env.YTLR_UPDATER_PUBLIC_KEY,
      repository,
    );
    await verifySigningSetup(config.plugins.updater.pubkey, version);
    const temp = await mkdtemp(join(tmpdir(), "ytlr-updater-"));
    try {
      const path = join(temp, "updater.conf.json");
      await writeFile(path, JSON.stringify(config)); // Public key only; private keys stay in the environment.
      run(process.execPath, [join(root, "packaging/prepare.mjs")]);
      run(
        process.execPath,
        [
          join(root, "apps/desktop/node_modules/@tauri-apps/cli/tauri.js"),
          "build",
          "--config",
          path,
        ],
        join(root, "apps/desktop"),
      );
    } finally {
      await rm(temp, { recursive: true, force: true });
    }
  } else if (command === "manifest") {
    const host = spawnSync("rustc", ["--print", "host-tuple"], {
      encoding: "utf8",
    });
    if (host.status !== 0) throw new Error("Cannot determine Rust target");
    const target = host.stdout.trim();
    if (
      process.env.YTLR_EXPECTED_UPDATE_PLATFORM &&
      platformForTarget(target) !== process.env.YTLR_EXPECTED_UPDATE_PLATFORM
    ) {
      throw new Error(
        "Build runner architecture does not match the expected updater platform",
      );
    }
    const manifest = await createManifest({
      bundleRoot: join(root, "target/release/bundle"),
      target,
      repository,
      tag,
      version,
      publicKey: process.env.YTLR_UPDATER_PUBLIC_KEY,
    });
    const directory = join(root, "target/release/update-manifests");
    await mkdir(directory, { recursive: true });
    await writeFile(
      join(directory, `${platformForTarget(target)}.json`),
      JSON.stringify(manifest, null, 2),
    );
  } else if (command === "merge") {
    const assets = resolve(process.argv[3] ?? "release-assets");
    const output = resolve(process.argv[4] ?? "latest.json");
    const names = await readdir(join(assets, "update-manifests"));
    const manifests = await Promise.all(
      names
        .filter((n) => n.endsWith(".json"))
        .map(async (name) =>
          JSON.parse(
            await readFile(join(assets, "update-manifests", name), "utf8"),
          ),
        ),
    );
    const manifest = mergeManifests(manifests);
    if (manifest.version !== version)
      throw new Error("Updater manifest does not match release tag");
    await verifyManifestAssets(manifest, assets, {
      repository,
      tag,
      publicKey: process.env.YTLR_UPDATER_PUBLIC_KEY,
    });
    await writeFile(output, JSON.stringify(manifest, null, 2));
  } else {
    throw new Error(
      "Usage: node packaging/updater.mjs build|manifest|merge [assets] [output]",
    );
  }
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
