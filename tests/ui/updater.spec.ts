import { test, expect } from "@playwright/test";
import type { UpdateStatus } from "../../apps/desktop/src/AppUpdates";

test("app updates show failures, progress, deferred installation and platform fallback", async ({
  page,
}) => {
  const status: UpdateStatus = {
    current_version: "0.2.3",
    available: true,
    auto_check: true,
    phase: "idle",
    version: null,
    notes: null,
    checked_at: null,
    downloaded: 0,
    total: null,
    message: null,
  };
  const snapshot = {
    version: "0.2.3",
    jobs: [],
    channels: [],
    storage: [],
    tools: [],
    installing_tools: false,
    tool_message: null,
    free_bytes: 10000000000,
    replica_targets: [],
    settings: {
      storage_root: "/fixture",
      max_recordings: 2,
      scan_interval_secs: 60,
      stall_timeout_secs: 120,
      min_free_bytes: 0,
      max_retries: 8,
      live_from_start: false,
      notifications: false,
      close_to_tray: true,
    },
  };
  let checks = 0,
    installs = 0,
    opened = 0;
  let finishDownload: () => void = () => {
    throw new Error("download not started");
  };
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.exposeFunction(
    "nativeInvoke",
    async (command: string, args: any) => {
      if (command === "set_ui_language") return null;
      if (command === "list_remotes") return [];
      if (command === "api" && args.path === "/snapshot") return snapshot;
      if (command === "update_status") return status;
      if (command === "set_update_preferences") {
        status.auto_check = args.autoCheck;
        return status;
      }
      if (command === "open_update_releases") {
        opened++;
        return null;
      }
      if (command === "check_app_update") {
        checks++;
        status.checked_at = new Date().toISOString();
        if (checks === 1) {
          status.phase = "error";
          status.message = "Network unavailable";
        } else {
          status.phase = "available";
          status.version = "0.3.0";
          status.notes = "New release <b>plain text</b>";
          status.message = null;
        }
        return status;
      }
      if (command === "install_app_update") {
        installs++;
        if (installs === 1) {
          status.phase = "downloading";
          status.downloaded = 25;
          status.total = 100;
          await new Promise<void>((resolve) => {
            finishDownload = resolve;
          });
          status.phase = "ready";
          status.downloaded = 100;
          status.message =
            "녹화 또는 파일 처리 중입니다. 완료 후 업데이트를 설치하세요.";
        } else {
          status.phase = "restarting";
          status.message = null;
        }
        return status;
      }
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
  const card = page.getByRole("region", { name: "App updates", exact: true });
  await card.getByLabel("Automatically check for updates").uncheck();
  await expect.poll(() => status.auto_check).toBe(false);
  await page.reload();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(
    card.getByLabel("Automatically check for updates"),
  ).not.toBeChecked();
  await card.getByRole("button", { name: "Check for updates now" }).click();
  await expect(
    card.getByText("Network unavailable", { exact: true }),
  ).toBeVisible();
  await card.getByRole("button", { name: "Check for updates now" }).click();
  await expect(
    card.getByText("A new version is available. (0.3.0)", { exact: true }),
  ).toBeVisible();
  await card.getByText("Release notes", { exact: true }).click();
  await expect(
    card.getByText("New release <b>plain text</b>", { exact: true }),
  ).toBeVisible();
  expect(await card.locator("b").count()).toBe(0);
  await card.getByRole("button", { name: "Download and install" }).click();
  await expect(card.getByRole("progressbar")).toHaveAttribute("value", "25");
  await expect(
    card.getByRole("button", { name: "Download and install" }),
  ).toBeDisabled();
  finishDownload();
  await expect(
    card.getByText(
      "Recording or file processing is active. Install the update after it finishes.",
      { exact: true },
    ),
  ).toBeVisible();
  await card.getByRole("button", { name: "Install and restart" }).click();
  await expect(
    card.getByText("Update installed · restarting app… (0.3.0)", {
      exact: true,
    }),
  ).toBeVisible();
  expect(installs).toBe(2);
  status.available = false;
  status.phase = "idle";
  status.version = null;
  status.message = "Linux 자동 업데이트는 AppImage에서 지원됩니다.";
  await expect(
    card.getByText("Automatic updates on Linux require AppImage.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    card.getByRole("button", { name: "Check for updates now" }),
  ).toBeDisabled();
  await card.getByRole("button", { name: "Open releases page" }).click();
  expect(opened).toBe(1);
  expect(errors).toEqual([]);
});
