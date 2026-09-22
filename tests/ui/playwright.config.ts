import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.ts",
  timeout: 60_000,
  workers: 1,
  use: {
    baseURL: "http://127.0.0.1:1420",
    viewport: { width: 1180, height: 800 },
    headless: true,
    screenshot: "only-on-failure",
  },
  webServer: {
    command: "pnpm --dir apps/desktop dev",
    cwd: "../..",
    url: "http://127.0.0.1:1420",
    timeout: 30_000,
  },
});
