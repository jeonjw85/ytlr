import { useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { api, openOperation } from "./api";
import { useI18n } from "./i18n";
import { bytes, type Job, type Operation } from "./types";

const taskLabels = {
  export: "내보내기",
  clip: "클립 만들기",
  range_clip: "구간 클립",
  preview: "미리보기 생성",
  recover: "원본 복구",
  cleanup: "원본 정리",
  download: "원격 다운로드",
};
const stateLabels = {
  queued: "작업 대기",
  running: "처리 중",
  completed: "작업 완료",
  failed: "작업 실패",
  cancelled: "취소됨",
};
export function Operations({
  operations,
  jobs,
  onChanged,
  remote,
}: {
  operations: Operation[];
  jobs: Job[];
  onChanged: () => Promise<void>;
  remote: boolean;
}) {
  const { t, dateLocale } = useI18n();
  const [downloads, setDownloads] = useState<Operation[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  useEffect(() => {
    if (!remote || !isTauri()) return;
    let disposed = false;
    const refresh = async () => {
      try {
        const value = await invoke<Operation[]>("local_downloads");
        if (!disposed) setDownloads(value);
      } catch (e) {
        if (!disposed) setError(String(e));
      }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 2000);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, [remote]);
  async function action(
    op: Operation,
    command: "cancel" | "retry",
    local: boolean,
  ) {
    setBusy(op.id);
    setError("");
    try {
      if (local) {
        await invoke("download_action", { id: op.id, action: command });
        setDownloads(await invoke("local_downloads"));
      } else {
        await api(`/operations/${op.id}/${command}`, "POST", {});
        await onChanged();
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }
  const rows = (items: Operation[], local: boolean) =>
    items.map((op) => (
      <div className="job-tool-section" key={op.id}>
        <strong>
          {t(taskLabels[op.request.task.kind])} ·{" "}
          {jobs.find((j) => j.id === op.request.job_id)?.title ??
            op.request.job_id}
        </strong>
        {("path" in op.request.task || op.request.source_path) && (
          <p className="muted">
            {"path" in op.request.task
              ? op.request.task.path
              : op.request.source_path}
          </p>
        )}
        {op.request.task.kind === "download" && (
          <p className="muted">{op.request.task.remote}</p>
        )}
        <p>
          {t(stateLabels[op.state])} ·{" "}
          {new Date(op.updated_at).toLocaleString(dateLocale)}
        </p>
        {op.state === "running" && (
          <progress
            aria-label={t("작업 진행률")}
            max="1"
            value={op.progress ?? undefined}
          />
        )}
        <p className="muted">
          {t(op.message)} {op.bytes_done > 0 && ` · ${bytes(op.bytes_done)}`}
          {op.request.task.kind === "download" &&
            op.total_bytes !== null &&
            ` / ${bytes(op.total_bytes)}`}
        </p>
        {op.error && <p className="inline-error">{t(op.error)}</p>}
        {op.result?.warnings?.map((warning, i) => (
          <p className="inline-error" key={i}>
            {t(warning)}
          </p>
        ))}
        {op.result?.pending_files?.map((path) => (
          <p className="muted" key={path}>
            {path}
          </p>
        ))}
        {op.result?.path && <p className="muted">{op.result.path}</p>}
        <div className="tool-actions">
          {(op.state === "queued" ||
            (op.state === "running" &&
              !["cleanup", "recover"].includes(op.request.task.kind))) && (
            <button
              className="button small"
              disabled={busy === op.id || op.cancel_requested}
              onClick={() => void action(op, "cancel", local)}
            >
              {t(op.cancel_requested ? "취소 요청됨" : "작업 취소")}
            </button>
          )}
          {["failed", "cancelled"].includes(op.state) && (
            <button
              className="button small"
              disabled={busy === op.id}
              onClick={() => void action(op, "retry", local)}
            >
              {t("작업 재시도")}
            </button>
          )}
          {op.state === "completed" &&
            op.result?.path &&
            (local || !remote) && (
              <button
                className="button small"
                onClick={() =>
                  void openOperation(
                    op.id,
                    local || op.request.task.kind === "download",
                  ).catch((e) => setError(String(e)))
                }
              >
                {t("결과 파일 열기")}
              </button>
            )}
        </div>
      </div>
    ));
  return (
    <section>
      <h2>{t("백그라운드 작업")}</h2>
      <p className="muted">
        {t(
          "작업은 서비스에 저장됩니다. 앱을 닫아도 계속 처리하며, 재시작 시 파일을 재검증합니다.",
        )}
      </p>
      {rows(operations, false)}
      {operations.length === 0 && <p>{t("등록된 작업이 없습니다.")}</p>}
      {remote && (
        <>
          <h3>{t("이 컴퓨터의 다운로드")}</h3>
          {rows(downloads, true)}
        </>
      )}
      {error && <p role="alert">{t(error)}</p>}
    </section>
  );
}
