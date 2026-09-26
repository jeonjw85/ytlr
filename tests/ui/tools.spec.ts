import { test, expect } from "@playwright/test";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import {
  mkdtemp,
  mkdir,
  readFile,
  writeFile,
  copyFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

let home: string,
  daemon: ChildProcess,
  endpoint: { port: number; token: string; api_version: number },
  job: any;
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function call(path: string, method = "GET", body?: unknown) {
  const response = await fetch(`http://127.0.0.1:${endpoint.port}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${endpoint.token}`,
      "x-ytlr-api-version": String(endpoint.api_version),
      "content-type": "application/json",
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const value = await response.json();
  if (!response.ok) throw new Error(value.error);
  return value;
}
test.beforeAll(async () => {
  home = await mkdtemp(join(tmpdir(), "ytlr-tools-ui-"));
  const cli = join(home, process.platform === "win32" ? "ytlr.exe" : "ytlr");
  await copyFile(
    resolve(`target/debug/ytlr${process.platform === "win32" ? ".exe" : ""}`),
    cli,
  );
  daemon = spawn(cli, ["--data-dir", home, "run", "--headless"], {
    stdio: "ignore",
  });
  for (let n = 0; n < 150; n++) {
    try {
      endpoint = JSON.parse(await readFile(join(home, "service.json"), "utf8"));
      await call("/health");
      break;
    } catch {
      await wait(200);
    }
  }
  const snapshot = await call("/snapshot");
  await call("/settings", "PUT", {
    ...snapshot.settings,
    min_free_bytes: 0,
    notifications: false,
    prevent_sleep: false,
    automation: {
      notifications: [
        { id: "ui-probe", kind: "webhook", url_env: "YTLR_UI_MISSING" },
      ],
      retention: {},
    },
  });
  job = await call("/jobs", "POST", {
    url: "https://youtu.be/toolpreview",
    schedule: { start_at: "2099-01-01T00:00:00Z" },
  });
  await call(`/jobs/${job.id}/stop`, "POST", {});
  await mkdir(job.output_dir, { recursive: true });
  expect(
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
      "8",
      "-c:v",
      "libx264",
      "-preset",
      "ultrafast",
      "-g",
      "10",
      "-c:a",
      "aac",
      join(job.output_dir, "export-ui.mp4"),
    ]).status,
  ).toBe(0);
});
test.afterAll(async () => {
  try {
    await call("/shutdown", "POST", {});
  } catch {}
  await wait(500);
  daemon?.kill();
});

test("startup preferences, notification testing, portable import and real preview/range export", async ({
  page,
}) => {
  let startup = { enabled: false, start_hidden: false };
  const opened: string[] = [];
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const exported = join(home, "settings.json");
  await page.exposeFunction(
    "nativeInvoke",
    async (command: string, args: any) => {
      if (command === "set_ui_language") return null;
      if (command === "list_remotes") return [];
      if (command === "api")
        return call(args.path, args.method, args.body ?? undefined);
      if (command === "update_status")
        return {
          current_version: "0.2.4",
          available: false,
          auto_check: false,
          phase: "idle",
          version: null,
          notes: null,
          checked_at: null,
          downloaded: 0,
          total: null,
          message: null,
        };
      if (command === "startup_status") return startup;
      if (command === "set_startup") {
        startup = { enabled: args.enabled, start_hidden: args.startHidden };
        return startup;
      }
      if (command === "plugin:dialog|save" || command === "plugin:dialog|open")
        return exported;
      if (command === "save_configuration") {
        await writeFile(args.path, JSON.stringify(args.bundle));
        return null;
      }
      if (command === "load_configuration") {
        const bundle = JSON.parse(await readFile(args.path, "utf8"));
        bundle.channels = [
          {
            url: "https://www.youtube.com/@imported/streams",
            name: "Imported channel",
            enabled: false,
            priority: 0,
            recording_options: { audio_only: false, max_height: 720 },
            live_from_start: false,
            rules: {
              include: ["concert"],
              exclude: [],
              window: null,
              duration_minutes: null,
            },
          },
        ];
        return bundle;
      }
      if (command === "media_url") {
        const { ticket } = await call(
          `/jobs/${args.id}/preview-ticket`,
          "POST",
          { path: args.path },
        );
        return `http://127.0.0.1:${endpoint.port}/play/${ticket}`;
      }
      if (command === "open_operation") {
        opened.push(args.id);
        return null;
      }
      if (command.includes("permission")) return false;
      throw new Error(`Unexpected native command ${command}`);
    },
  );
  await page.addInitScript(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      value: {
        invoke: (command: string, args: unknown) =>
          (window as any).nativeInvoke(command, args),
      },
    });
    Object.defineProperty(window, "isTauri", { value: true });
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByLabel("Start the recording app at login").check();
  await page.getByLabel("Start minimized to tray at login").check();
  await expect
    .poll(() => startup)
    .toEqual({ enabled: true, start_hidden: true });
  await page
    .getByRole("button", {
      name: "Notification connections and history",
      exact: true,
    })
    .click();
  await expect(page.getByText(/Environment variable missing/)).toBeVisible();
  await page
    .getByRole("button", { name: "Send test notification", exact: true })
    .click();
  await expect
    .poll(async () =>
      (await call("/notifications")).some((d: any) => d.attempts > 0),
    )
    .toBe(true);
  await page
    .getByRole("button", {
      name: "Notification connections and history",
      exact: true,
    })
    .click();
  await expect(
    page.getByRole("button", { name: "Retry delivery now", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Export configuration", exact: true })
    .click();
  await expect(
    page.getByText("Configuration backup saved.", { exact: true }),
  ).toBeVisible();
  await expect
    .poll(
      async () => JSON.parse(await readFile(exported, "utf8")).format_version,
    )
    .toBe(1);
  await page
    .getByRole("button", { name: "Open configuration file", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Preview import", exact: true })
    .click();
  await expect(
    page.getByText("Channels added / replaced / skipped: 1 / 0 / 0", {
      exact: true,
    }),
  ).toBeVisible();
  expect((await call("/snapshot")).channels).toHaveLength(0);
  await page.getByRole("button", { name: "Apply import", exact: true }).click();
  await expect
    .poll(async () => (await call("/snapshot")).channels.length)
    .toBe(1);
  await page.getByRole("button", { name: "Library", exact: true }).click();
  await page
    .locator(".library-row")
    .getByRole("button", { name: "Details", exact: true })
    .click();
  await expect(
    page.getByLabel("Preview source file", { exact: true }),
  ).toHaveValue("export-ui.mp4");
  await page
    .getByRole("button", { name: "Create preview file", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await call("/operations")).some(
          (o: any) =>
            o.request.task.kind === "preview" && o.state === "completed",
        ),
      { timeout: 20000 },
    )
    .toBe(true);
  const video = page.locator("video");
  await expect(video).toBeVisible();
  await expect
    .poll(() => video.evaluate((v: HTMLVideoElement) => v.readyState), {
      timeout: 10000,
    })
    .toBeGreaterThan(0);
  await video.evaluate((v: HTMLVideoElement) => {
    v.currentTime = 2;
  });
  await page
    .getByRole("button", { name: "Use current position as start", exact: true })
    .click();
  await page.getByLabel("Clip end (seconds)", { exact: true }).fill("4");
  await page
    .getByRole("button", { name: "Export selected range", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await call("/operations")).some(
          (o: any) =>
            o.request.task.kind === "range_clip" && o.state === "completed",
        ),
      { timeout: 20000 },
    )
    .toBe(true);
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Tasks", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Background tasks", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Open result file", exact: true })
    .first()
    .click();
  await expect.poll(() => opened.length).toBe(1);
  expect(errors).toEqual([]);
});
