import { createHash, createPublicKey, verify } from "node:crypto";
import { createReadStream } from "node:fs";

function base64(value, label) {
  if (
    typeof value !== "string" ||
    !value ||
    !/^[A-Za-z0-9+/]+={0,2}$/.test(value)
  ) {
    throw new Error(`Invalid ${label} encoding`);
  }
  const bytes = Buffer.from(value, "base64");
  if (bytes.toString("base64") !== value)
    throw new Error(`Invalid ${label} encoding`);
  return bytes;
}

export function publicKeyBytes(publicKey) {
  const lines = base64(publicKey?.trim(), "public key")
    .toString("utf8")
    .trim()
    .split(/\r?\n/);
  const raw = base64(lines[1], "public key");
  if (
    lines.length !== 2 ||
    !lines[0].startsWith("untrusted comment:") ||
    raw.length !== 42 ||
    raw.subarray(0, 2).toString() !== "Ed"
  ) {
    throw new Error(
      "Set YTLR_UPDATER_PUBLIC_KEY to the contents of the Tauri .pub file",
    );
  }
  return raw;
}

// Tauri's current signer uses Minisign prehashed Ed25519 signatures. OpenSSL's
// Ed25519 and BLAKE2b primitives verify both the artifact and trusted version
// comment; deployment deliberately rejects legacy, unversioned signatures.
export async function verifyArtifactSignature(
  path,
  encodedSignature,
  publicKey,
  version,
) {
  const key = publicKeyBytes(publicKey);
  const lines = base64(encodedSignature.trim(), "signature")
    .toString("utf8")
    .trim()
    .split(/\r?\n/);
  if (
    lines.length !== 4 ||
    !lines[0].startsWith("untrusted comment:") ||
    !lines[2].startsWith("trusted comment: ")
  ) {
    throw new Error("Invalid updater signature format");
  }
  const signature = base64(lines[1], "signature");
  const globalSignature = base64(lines[3], "trusted comment signature");
  if (
    signature.length !== 74 ||
    signature.subarray(0, 2).toString() !== "ED" ||
    globalSignature.length !== 64
  ) {
    throw new Error("Expected a current Tauri prehashed signature");
  }
  if (!signature.subarray(2, 10).equals(key.subarray(2, 10)))
    throw new Error("Updater signing key does not match the public key");
  const comment = lines[2].slice("trusted comment: ".length);
  const versions = comment
    .split("\t")
    .filter((field) => field.startsWith("version:"))
    .map((field) => field.slice("version:".length));
  if (versions.length !== 1 || versions[0] !== version)
    throw new Error("Updater signature version does not match the release");
  const verifierKey = createPublicKey({
    key: Buffer.concat([
      Buffer.from("302a300506032b6570032100", "hex"),
      key.subarray(10),
    ]),
    format: "der",
    type: "spki",
  });
  const hash = createHash("blake2b512");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  const artifactSignature = signature.subarray(10);
  if (
    !verify(null, hash.digest(), verifierKey, artifactSignature) ||
    !verify(
      null,
      Buffer.concat([artifactSignature, Buffer.from(comment)]),
      verifierKey,
      globalSignature,
    )
  ) {
    throw new Error(
      "Updater artifact or trusted comment signature verification failed",
    );
  }
}
