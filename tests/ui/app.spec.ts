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
  await page.getByRole("button", { name: "추가하기", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".job-card")).toHaveCount(1);
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
  await expect(page.getByText("중지됨", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "채널", exact: true }).click();
  await page
    .getByRole("button", { name: "채널 추가", exact: true })
    .first()
    .click();
  await page.getByLabel("채널 URL").fill("https://youtube.com/@example");
  await page.getByLabel("표시 이름").fill("내 채널");
  await page.getByRole("button", { name: "추가하기", exact: true }).click();
  await expect(page.locator(".channel-card")).toHaveCount(1);
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
