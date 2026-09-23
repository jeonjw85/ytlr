import { test, expect } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

let daemon: ChildProcess;
let endpoint: { port: number; token: string };
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

test.beforeAll(async () => {
  const home = await mkdtemp(join(tmpdir(), "ytlr-ui-"));
  daemon = spawn(
    resolve("target/debug/ytlr"),
    ["--data-dir", home, "run", "--headless"],
    { stdio: "ignore" },
  );
  for (let i = 0; i < 150; i++) {
    try {
      endpoint = JSON.parse(await readFile(join(home, "service.json"), "utf8"));
      await call("/health");
      break;
    } catch {
      await wait(200);
    }
  }
  const snap = await call("/snapshot");
  // No external network recording in UI tests. The queue is held by the disk budget.
  await call("/settings", "PUT", {
    ...snap.settings,
    min_free_bytes: 9_000_000_000_000,
  });
});
test.afterAll(async () => {
  try {
    await call("/shutdown", "POST", {});
  } finally {
    await wait(500);
    daemon?.kill();
  }
});
async function call(path: string, method = "GET", body?: unknown) {
  const response = await fetch(`http://127.0.0.1:${endpoint.port}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${endpoint.token}`,
      "x-ytlr-api-version": String(endpoint.api_version),
      "content-type": "application/json",
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const value = await response.json();
  if (!response.ok) throw new Error(value.error);
  return value;
}

test("GUI persists language and controls the real Rust service: create, deduplicate, stop, channels, settings", async ({
  page,
}) => {
  const errors: string[] = [];
  const nativeLanguages: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.exposeFunction(
    "nativeInvoke",
    async (command: string, args: any) => {
      if (command === "set_ui_language") {
        nativeLanguages.push(args.language);
        return null;
      }
      if (command === "list_remotes")
        return [
          {
            id: "offline",
            name: "offline",
            ssh: "test@offline",
            remote_data_dir: "/test",
          },
        ];
      if (command === "connect_remote") {
        if (args.name) throw new Error("원격 연결 실패");
        return "local";
      }
      if (command === "api") {
        if (args.expectedTarget !== "local") throw new Error("원격 연결 없음");
        return call(args.path, args.method, args.body ?? undefined);
      }
      if (command.includes("is_permission_granted")) return false;
      if (command.includes("permission")) return "denied";
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
  await expect(page).toHaveTitle("YTLR");
  await expect(page.locator(".brand")).toHaveText("YTLR");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(
    page.getByRole("button", { name: "New recording", exact: true }),
  ).toBeEnabled();
  await expect(page.locator(".local-status")).toContainText("Recording device");
  await page
    .getByRole("button", { name: "New recording", exact: true })
    .click();
  await expect(
    page.getByRole("dialog", { name: "New live recording" }),
  ).toBeVisible();
  await expect(page.getByLabel("Video URL")).toBeVisible();
  await expect(page.getByLabel("Maximum quality")).toHaveValue("best");
  await page.getByLabel("Recording mode").selectOption("audio");
  await expect(page.getByLabel("Maximum quality")).toHaveCount(0);
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByLabel("Concurrent recordings")).toBeVisible();
  await page.screenshot({
    path: "test-results/settings-en.png",
    fullPage: true,
  });
  await page.getByLabel("Language", { exact: true }).selectOption("ko");
  await expect(page.locator("html")).toHaveAttribute("lang", "ko");
  await expect(
    page.getByRole("heading", { name: "설정", exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "test-results/settings-ko.png",
    fullPage: true,
  });
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("lang", "ko");
  await page.getByRole("button", { name: "설정", exact: true }).click();
  await expect(page.getByLabel("언어", { exact: true })).toHaveValue("ko");
  await page.getByLabel("언어", { exact: true }).selectOption("en");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByLabel("Language", { exact: true }).selectOption("ko");
  await page.getByRole("button", { name: "녹화", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "새 녹화", exact: true }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "새 녹화", exact: true }).click();
  await page
    .getByLabel("영상 URL")
    .fill("https://www.youtube.com/watch?v=abcdefghijk");
  await page.getByLabel("최대 화질").selectOption("720");
  await page.getByLabel("종료 예약", { exact: true }).selectOption("minutes");
  await page.getByLabel("종료까지 남은 분").fill("30");
  await page.getByRole("button", { name: "추가하기", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".job-card")).toHaveCount(1);
  expect((await call("/snapshot")).jobs[0].recording_options).toEqual({
    audio_only: false,
    max_height: 720,
  });
  expect(Date.parse((await call("/snapshot")).jobs[0].stop_at)).toBeGreaterThan(
    Date.now(),
  );
  await expect(page.getByText(/예약 종료까지/)).toBeVisible();
  await page
    .locator(".job-card")
    .getByRole("button", { name: "상세", exact: true })
    .click();
  await page
    .getByRole("button", { name: "종료 예약 변경", exact: true })
    .click();
  await page.getByLabel("종료 예약", { exact: true }).selectOption("none");
  await page.getByRole("button", { name: "예약 저장", exact: true }).click();
  await expect
    .poll(async () => (await call("/snapshot")).jobs[0].stop_at)
    .toBeNull();
  await page.keyboard.press("Escape");
  await expect(
    page.getByText("저장 공간 부족 경고", { exact: true }),
  ).toBeVisible({ timeout: 15000 });
  await page.getByRole("button", { name: "새 녹화", exact: true }).click();
  await page.getByLabel("영상 URL").fill("https://youtu.be/abcdefghijk");
  await page.getByRole("button", { name: "추가하기", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".job-card")).toHaveCount(1);
  await page.screenshot({ path: "test-results/dashboard.png", fullPage: true });
  await page.getByRole("button", { name: "중지", exact: true }).click();
  await expect(page.locator(".job-card")).toHaveCount(0);
  await page.getByRole("button", { name: "보관함", exact: true }).click();
  await expect(page.locator(".library-row")).toHaveCount(1);
  await expect(page.getByText("저장됨", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "채널", exact: true }).click();
  await page
    .getByRole("button", { name: "채널 추가", exact: true })
    .first()
    .click();
  await page.getByLabel("채널 URL").fill("https://youtube.com/@example");
  await page.getByLabel("표시 이름").fill("내 채널");
  await page.getByLabel("녹화 모드").selectOption("audio");
  await page.getByRole("button", { name: "추가하기", exact: true }).click();
  await expect(page.locator(".channel-card")).toHaveCount(1);
  expect(
    (await call("/snapshot")).channels[0].recording_options.audio_only,
  ).toBe(true);
  await page.getByRole("button", { name: "녹화 옵션", exact: true }).click();
  await page.getByLabel("녹화 모드").selectOption("video");
  await page.getByLabel("최대 화질").selectOption("480");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect((await call("/snapshot")).channels[0].recording_options).toEqual({
    audio_only: false,
    max_height: 480,
  });
  const toggle = page.getByRole("switch", { name: "내 채널 자동 녹화" });
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  await page.getByRole("button", { name: "설정", exact: true }).click();
  await page.getByLabel("동시 녹화 수").fill("3");
  // Changing UI language must not discard unsaved recording settings.
  await page.getByLabel("언어", { exact: true }).selectOption("en");
  await expect(page.getByLabel("Concurrent recordings")).toHaveValue("3");
  await expect(
    page.getByRole("button", { name: "Save settings", exact: true }),
  ).toBeEnabled();
  await page.getByLabel("Language", { exact: true }).selectOption("ko");
  await page.getByRole("button", { name: "설정 저장", exact: true }).click();
  await expect(page.getByLabel("동시 녹화 수")).toHaveValue("3");
  await expect
    .poll(async () => (await call("/snapshot")).settings.max_recordings)
    .toBe(3);
  expect(errors).toEqual([]);
  await page.getByLabel("연결 대상").selectOption("offline");
  await expect(page.getByLabel("연결 대상")).toHaveValue("offline");
  await expect(
    page.getByText("서비스에 연결할 수 없습니다", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "녹화", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "새 녹화", exact: true }),
  ).toBeDisabled();
  await expect(page.locator(".job-card")).toHaveCount(0);
  await page.getByRole("button", { name: "설정", exact: true }).click();
  await page.getByLabel("언어", { exact: true }).selectOption("en");
  await expect(
    page.getByText("Cannot connect to the service", { exact: true }),
  ).toBeVisible();
  await page.getByLabel("Language", { exact: true }).selectOption("ko");
  await page.getByRole("button", { name: "녹화", exact: true }).click();
  await page.getByLabel("연결 대상").selectOption("local");
  await expect(
    page.getByRole("button", { name: "새 녹화", exact: true }),
  ).toBeEnabled();
  expect(nativeLanguages).toContain("en");
  expect(nativeLanguages).toContain("ko");
  expect(errors).toEqual([]);
});

test("bookmark editing, selected cleanup, and deduplicated recovery notifications", async ({
  page,
}) => {
  const base = await call("/snapshot");
  const template = await call("/jobs", "POST", {
    url: "https://youtu.be/uifixture01",
  });
  const job = {
    ...template,
    id: "ui-recording",
    title: "UI live fixture",
    state: "recording",
    stop_requested: false,
    attempt: 2,
    last_media_at: new Date().toISOString(),
    stop_at: null,
    continuity_uncertain: false,
    bookmarks: [] as any[],
    alerts: [] as any[],
    outputs: [] as any[],
  };
  const snapshot = {
    ...base,
    jobs: [job],
    channels: [],
    storage: [],
    settings: { ...base.settings, notifications: true },
  };
  const notifications: any[] = [];
  await page.exposeFunction(
    "captureNotification",
    (title: string, options: unknown) => notifications.push({ title, options }),
  );
  await page.addInitScript(() => {
    class TestNotification {
      static permission = "granted";
      constructor(title: string, options: unknown) {
        void (window as any).captureNotification(title, options);
      }
    }
    Object.defineProperty(window, "Notification", { value: TestNotification });
  });
  const cleanupRequests: any[] = [];
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.exposeFunction(
    "nativeInvoke",
    async (command: string, args: any) => {
      if (command === "set_ui_language" || command === "open_job") return null;
      if (command === "list_remotes") return [];
      if (command.includes("is_permission_granted")) return true;
      if (command.includes("notify")) {
        notifications.push(args);
        return null;
      }
      if (command === "api") {
        if (args.path === "/snapshot") return snapshot;
        if (args.path.endsWith("/events")) return [];
        if (args.path.endsWith("/bookmarks") && args.method === "POST") {
          job.bookmarks.push({
            id: "mark-1",
            title: args.body.title,
            note: "",
            attempt: 2,
            media_seconds: 42.5,
            created_at: new Date().toISOString(),
            received_at: new Date().toISOString(),
          });
          return job;
        }
        if (args.path.endsWith("/bookmarks/mark-1")) {
          if (args.method === "DELETE") job.bookmarks = [];
          else Object.assign(job.bookmarks[0], args.body);
          return job;
        }
        if (args.path.endsWith("/storage"))
          return { results: 2000, segments: 2000, sources: 500 };
        if (args.path.endsWith("/cleanup/preview"))
          return {
            id: "plan-1",
            job_id: job.id,
            created_at: new Date().toISOString(),
            retained: job.outputs,
            reclaimable_bytes: 2000,
            files: [
              { path: "attempt-0002/part-001.mkv", bytes: 1000, sha256: "a" },
              { path: "attempt-0002/part-002.mkv", bytes: 1000, sha256: "b" },
            ],
          };
        if (args.path.endsWith("/cleanup")) {
          cleanupRequests.push(args.body);
          return { reclaimed_bytes: 1000 };
        }
      }
      throw new Error(
        `Unexpected native command ${command}: ${JSON.stringify(args)}`,
      );
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
  await page
    .locator(".job-card")
    .getByRole("button", { name: "Mark now", exact: true })
    .click();
  await expect.poll(() => job.bookmarks.length).toBe(1);
  await page
    .locator(".job-card")
    .getByRole("button", { name: "Details", exact: true })
    .click();
  await expect(page.getByText(/00:00:42/)).toBeVisible();
  await page
    .getByRole("button", { name: "Edit bookmark", exact: true })
    .click();
  await page.getByLabel("Bookmark title").fill("Chorus");
  await page.getByLabel("Bookmark note").fill("Keep this section");
  await page
    .getByRole("button", { name: "Save bookmark", exact: true })
    .click();
  await expect(
    page.getByText("Keep this section", { exact: true }),
  ).toBeVisible();
  expect(job.bookmarks[0].media_seconds).toBe(42.5);
  job.alerts = [
    {
      id: "incident-1",
      kind: "stalled",
      message: "미디어 수신이 멈췄습니다.",
      active: true,
      opened_at: new Date().toISOString(),
      resolved_at: null,
    },
  ];
  await expect.poll(() => notifications.length).toBe(1);
  await page.waitForTimeout(4200);
  expect(notifications.length).toBe(1);
  job.alerts[0].active = false;
  job.alerts[0].resolved_at = new Date().toISOString();
  await expect.poll(() => notifications.length).toBe(2);
  expect(notifications[0].title).toContain("Recording problem");
  expect(notifications[1].title).toContain("Recording alert cleared");
  await page.waitForTimeout(2200);
  expect(notifications.length).toBe(2);
  job.state = "stopped";
  job.outputs = [
    {
      path: "/fixture/attempt-0002/recording-result.mkv",
      bytes: 2000,
      duration: 60,
      has_video: true,
      has_audio: true,
      verification: "container_probe",
    },
  ];
  await page
    .getByRole("button", { name: "Preview cleanup", exact: true })
    .click();
  await expect(page.getByText(/Results to keep/)).toBeVisible();
  await page
    .getByLabel("attempt-0002/part-002.mkv", { exact: false })
    .uncheck();
  page.on("dialog", (dialog) => void dialog.accept());
  await page
    .getByRole("button", {
      name: "Permanently delete selected files",
      exact: true,
    })
    .click();
  await expect.poll(() => cleanupRequests.length).toBe(1);
  expect(cleanupRequests[0]).toEqual({
    plan_id: "plan-1",
    files: ["attempt-0002/part-001.mkv"],
  });
  await page
    .getByRole("button", { name: "Delete bookmark", exact: true })
    .click();
  await expect.poll(() => job.bookmarks.length).toBe(0);
  expect(errors).toEqual([]);
});
