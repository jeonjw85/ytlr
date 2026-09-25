import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { useI18n } from "./i18n";
import { bytes } from "./types";

export interface UpdateStatus {
  current_version: string;
  available: boolean;
  auto_check: boolean;
  phase:
    | "idle"
    | "checking"
    | "available"
    | "current"
    | "downloading"
    | "ready"
    | "installing"
    | "restarting"
    | "error";
  version: string | null;
  notes: string | null;
  checked_at: string | null;
  downloaded: number;
  total: number | null;
  message: string | null;
}
export function downloadPercent(
  status: Pick<UpdateStatus, "downloaded" | "total">,
): number | null {
  return status.total && status.total > 0
    ? Math.min(
        100,
        Math.max(0, Math.floor((status.downloaded / status.total) * 100)),
      )
    : null;
}
const labels: Record<UpdateStatus["phase"], string> = {
  idle: "새 버전을 확인할 수 있습니다.",
  checking: "업데이트 확인 중…",
  available: "새 버전을 설치할 수 있습니다.",
  current: "최신 버전입니다.",
  downloading: "업데이트 다운로드·서명 검증 중…",
  ready: "다운로드 완료 · 설치 대기",
  installing: "로컬 서비스 종료·업데이트 설치 중…",
  restarting: "업데이트 완료 · 앱 재시작 중…",
  error: "업데이트를 완료하지 못했습니다.",
};

export function AppUpdates({ settings }: { settings: boolean }) {
  const { t, dateLocale } = useI18n();
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [autoCheckDraft, setAutoCheckDraft] = useState<boolean | null>(null);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const statusEpoch = useRef(0);
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let fetching = false;
    const refresh = async () => {
      if (fetching) return;
      fetching = true;
      const epoch = statusEpoch.current;
      try {
        const value = await invoke<UpdateStatus>("update_status");
        if (!disposed && epoch === statusEpoch.current) setStatus(value);
      } catch (e) {
        if (!disposed && epoch === statusEpoch.current) setError(String(e));
      } finally {
        fetching = false;
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, []);
  const working =
    busy ||
    (status &&
      ["checking", "downloading", "installing", "restarting"].includes(
        status.phase,
      ));
  async function action(command: string, args?: Record<string, unknown>) {
    statusEpoch.current++;
    setBusy(true);
    setError("");
    try {
      const value = await invoke<UpdateStatus>(command, args);
      statusEpoch.current++;
      setStatus(value);
    } catch (e) {
      statusEpoch.current++;
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  if (
    !isTauri() ||
    (!settings && (!status?.version || dismissed === status.version))
  )
    return null;
  const percent = status ? downloadPercent(status) : null;
  return (
    <section
      className="settings-card app-updates"
      aria-label={t("앱 업데이트")}
    >
      <div className="section-bar">
        <h2>
          {t("앱 업데이트")}
          {status && ` · ${status.current_version}`}
        </h2>
        {!settings && (
          <button
            className="button small"
            disabled={!!working}
            onClick={() => setDismissed(status?.version ?? null)}
          >
            {t("업데이트 알림 닫기")}
          </button>
        )}
      </div>
      {status && (
        <>
          <p role="status">
            {t(labels[status.phase])}
            {status.version && ` (${status.version})`}
          </p>
          {status.phase === "downloading" && (
            <div>
              <progress
                aria-label={t("업데이트 다운로드 진행률")}
                max={100}
                value={percent ?? undefined}
              />
              <p>
                {bytes(status.downloaded)}
                {status.total ? ` / ${bytes(status.total)}` : ""}
                {percent !== null ? ` · ${percent}%` : ""}
              </p>
            </div>
          )}
          {status.message && (
            <p className="inline-error">{t(status.message)}</p>
          )}
          {settings && (
            <>
              <label className="check-field">
                <input
                  type="checkbox"
                  disabled={!!working}
                  checked={autoCheckDraft ?? status.auto_check}
                  onChange={(e) => {
                    const autoCheck = e.target.checked;
                    setAutoCheckDraft(autoCheck);
                    void action("set_update_preferences", {
                      autoCheck,
                    }).finally(() => setAutoCheckDraft(null));
                  }}
                />
                {t("새 버전 자동 확인")}
              </label>
              <p className="muted">
                {t(
                  "앱 시작 후와 6시간마다 확인합니다. 설치는 버튼을 눌렀을 때 진행하며, 녹화·파일 처리 중이면 보류합니다.",
                )}
              </p>
              <p className="muted">
                {t(
                  "이 컴퓨터의 앱과 로컬 녹화 서비스를 업데이트합니다. 원격 서버는 별도로 업데이트하세요.",
                )}
              </p>
              {status.checked_at && (
                <p className="muted">
                  {t("마지막 업데이트 확인")} ·{" "}
                  {new Date(status.checked_at).toLocaleString(dateLocale)}
                </p>
              )}
            </>
          )}
          {status.notes && (
            <details>
              <summary>{t("릴리스 변경 사항")}</summary>
              <pre className="automation-log">{status.notes}</pre>
            </details>
          )}
          <div className="tool-actions">
            {settings && (
              <button
                className="button"
                disabled={!!working || !status.available}
                onClick={() => void action("check_app_update")}
              >
                {t("지금 업데이트 확인")}
              </button>
            )}
            {status.version && status.available && (
              <button
                className="button primary"
                disabled={!!working}
                onClick={() => void action("install_app_update")}
              >
                {t(
                  status.phase === "ready"
                    ? "설치 및 재시작"
                    : "다운로드 및 설치",
                )}
              </button>
            )}
            <button
              className="button"
              onClick={() =>
                void invoke("open_update_releases").catch((e) =>
                  setError(String(e)),
                )
              }
            >
              {t("릴리스 페이지 열기")}
            </button>
          </div>
        </>
      )}
      {error && (
        <p className="inline-error" role="alert">
          {t(error)}
        </p>
      )}
    </section>
  );
}
