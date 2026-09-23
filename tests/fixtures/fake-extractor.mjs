#!/usr/bin/env node
// Deterministic process-boundary fixture. Media data is a real FFmpeg-generated file.
import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);
const home = process.env.YTLR_FIXTURE;
if (args.includes("--version")) {
  if (home && fs.existsSync(path.join(home, "version-fail-once"))) {
    fs.rmSync(path.join(home, "version-fail-once"));
    process.exit(1);
  }
  console.log("fixture-1");
  process.exit(0);
}
if (!home) throw new Error("YTLR_FIXTURE is required");
const url = args.at(-1);
if (fs.existsSync(path.join(home, "require-auth"))) {
  const configPath = args[args.indexOf("--config-locations") + 1];
  if (!configPath || configPath.includes("recordings"))
    throw new Error("private auth config is required");
  const config = fs.readFileSync(configPath, "utf8");
  const cookiePath = JSON.parse(
    config.match(/^--cookies (".*")$/m)?.[1] ?? "null",
  );
  if (
    !cookiePath ||
    !config.includes(
      "youtube:player_client=mweb;po_token=mweb.gvs+TEST_PO_TOKEN_abcdefghijklmnop",
    )
  )
    throw new Error("auth arguments missing");
  const cookie = fs.readFileSync(cookiePath, "utf8");
  if (!cookie.includes("TEST_COOKIE_SECRET"))
    throw new Error("cookie copy missing");
  fs.appendFileSync(cookiePath, "# changed by extractor\n");
  fs.appendFileSync(path.join(home, "auth-checked"), "checked\n");
  console.error(
    "WARNING: fixture TEST_PO_TOKEN_abcdefghijklmnop TEST_COOKIE_SECRET",
  );
}
const id = new URL(url).searchParams.get("v") ?? "channel";
const counts = path.join(home, `${id}.count`);
if (args.includes("--flat-playlist")) {
  console.log(
    JSON.stringify({
      entries: [{ id: "qqqqwwwweee", live_status: "is_upcoming" }],
    }),
  );
  process.exit(0);
}
if (args.includes("--dump-single-json")) {
  if (process.env.YTLR_SOAK_SOURCE) {
    console.log(
      JSON.stringify({
        id,
        title: `Soak ${id}`,
        channel: "Local HLS",
        live_status: "is_live",
        format: "H264/AAC",
        requested_formats: [
          {
            format_id: "av",
            url: `${process.env.YTLR_SOAK_SOURCE}/${id}/live.m3u8`,
            vcodec: "h264",
            acodec: "aac",
          },
        ],
      }),
    );
    process.exit(0);
  }
  let count = 0;
  try {
    count = Number(fs.readFileSync(counts, "utf8"));
  } catch {}
  fs.writeFileSync(counts, String(count + 1));
  console.log(
    JSON.stringify({
      id,
      title: `Fixture ${id}`,
      channel: "Test channel",
      format: "test highest",
      live_status:
        id === "qqqqwwwweee"
          ? "is_upcoming"
          : id === "ffffgggghhh" && count > 0
            ? "was_live"
            : "is_live",
      requested_formats:
        id === "ffffgggghhh"
          ? [
              {
                format_id: "av",
                url: process.env.YTLR_FIXTURE_MEDIA,
                vcodec: "h264",
                acodec: "aac",
              },
            ]
          : [
              {
                format_id: "video",
                url: process.env.YTLR_FIXTURE_MEDIA,
                vcodec: "h264",
                acodec: "none",
              },
              {
                format_id: "audio",
                url: process.env.YTLR_FIXTURE_MEDIA,
                vcodec: "none",
                acodec: "aac",
              },
            ],
    }),
  );
  process.exit(0);
}
const template = args[args.indexOf("--output") + 1];
const output = template.replace("%(ext)s", "mkv");
const folder = path.dirname(output);
fs.mkdirSync(folder, { recursive: true });
const marker = path.join(home, `active-${process.pid}`);
fs.writeFileSync(marker, String(process.pid));
console.log(
  "__YTLR_INFO__" +
    JSON.stringify({
      title: `Fixture ${id}`,
      channel: "Test channel",
      format: "test highest",
    }),
);
const media = fs.readFileSync(path.join(home, "sample.mkv"));
fs.writeFileSync(`${output}.part`, media);
let n = 0;
const timer = setInterval(() => {
  n++;
  fs.writeFileSync(`${output}.part-Frag${n}`, media);
  for (const track of ["video", "audio"])
    console.log(
      "[download] __YTLR_PROGRESS__" +
        JSON.stringify({
          bytes: n * media.length,
          fragment: n,
          track,
          status: "downloading",
        }),
    );
}, 300);
function stop() {
  clearInterval(timer);
  fs.renameSync(`${output}.part`, output);
  fs.rmSync(marker, { force: true });
  process.exit(0);
}
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
