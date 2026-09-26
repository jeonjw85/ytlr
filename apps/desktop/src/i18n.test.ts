/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import appSource from "./App.tsx?raw";
import jobToolsSource from "./JobTools.tsx?raw";
import automationSource from "./Automation.tsx?raw";
import librarySource from "./Library.tsx?raw";
import clipsSource from "./Clips.tsx?raw";
import updatesSource from "./AppUpdates.tsx?raw";
import operationsSource from "./Operations.tsx?raw";
import startupSource from "./Startup.tsx?raw";
import configurationSource from "./Configuration.tsx?raw";
import mediaSource from "./MediaLibrary.tsx?raw";
import notificationsSource from "./NotificationHistory.tsx?raw";
import ts from "typescript";
import { translate } from "./i18n";
import { english } from "./locales/en";
import { stateLabels } from "./types";

describe("UI translations", () => {
  it("keeps parameter values and unknown diagnostics intact", () => {
    expect(
      translate("en", "{name} 자동 녹화", { name: "내 채널 {name}" }),
    ).toBe("Auto-record 내 채널 {name}");
    expect(translate("ko", "{seconds}초 전", { seconds: 8 })).toBe("8초 전");
    expect(translate("en", "__proto__")).toBe("__proto__");
    expect(translate("en", "Unexpected response: 503")).toBe(
      "Unexpected response: 503",
    );
  });

  it("covers UI copy and state labels in the English catalog", () => {
    const source = ts.createSourceFile(
      "App.tsx",
      appSource,
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TSX,
    );
    const missing: string[] = [];
    const visit = (node: ts.Node) => {
      if (
        ts.isStringLiteralLike(node) &&
        /[가-힣]/.test(node.text) &&
        !Object.hasOwn(english, node.text)
      )
        missing.push(node.text);
      if (
        ts.isJsxText(node) &&
        /[가-힣]/.test(node.text) &&
        node.text.trim() !== "한국어"
      )
        missing.push(`Untranslated JSX: ${node.text.trim()}`);
      ts.forEachChild(node, visit);
    };
    visit(source);
    for (const text of [
      automationSource,
      librarySource,
      clipsSource,
      updatesSource,
      operationsSource,
      startupSource,
      configurationSource,
      mediaSource,
      notificationsSource,
    ]) {
      visit(
        ts.createSourceFile(
          "Feature.tsx",
          text,
          ts.ScriptTarget.Latest,
          true,
          ts.ScriptKind.TSX,
        ),
      );
    }
    visit(
      ts.createSourceFile(
        "JobTools.tsx",
        jobToolsSource,
        ts.ScriptTarget.Latest,
        true,
        ts.ScriptKind.TSX,
      ),
    );
    for (const label of Object.values(stateLabels)) {
      if (!Object.hasOwn(english, label)) missing.push(label);
    }
    expect(missing).toEqual([]);
  });
});
