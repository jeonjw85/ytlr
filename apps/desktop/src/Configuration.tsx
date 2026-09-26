import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, scopedApi } from "./api";
import { useI18n } from "./i18n";

interface ImportReport {
  added_channels: number;
  replaced_channels: number;
  skipped_channels: number;
  added_schedules: number;
  skipped_schedules: number;
  expired_schedules: number;
  storage_root: string;
  retention: {
    cleanup_after_days: number | null;
    delete_after_days: number | null;
  };
}
export function Configuration({
  onChanged,
  root,
}: {
  onChanged: () => Promise<void>;
  root: string;
}) {
  const { t } = useI18n();
  const [bundle, setBundle] = useState<unknown>(null);
  const [replace, setReplace] = useState(false);
  const [settings, setSettings] = useState(false);
  const [storage, setStorage] = useState(root);
  const [report, setReport] = useState<ImportReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const request = () => ({
    bundle,
    replace_channels: replace,
    apply_settings: settings,
    storage_root: storage || null,
  });
  async function exportFile() {
    const request = scopedApi();
    setBusy(true);
    setMessage("");
    try {
      const path = await save({
        defaultPath: "ytlr-settings.json",
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (path) {
        const data = await request("/configuration/export");
        await invoke("save_configuration", { path, bundle: data });
        setMessage(t("설정 백업을 저장했습니다."));
      }
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function load() {
    setBusy(true);
    setMessage("");
    setReport(null);
    try {
      const path = await open({
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (path) {
        setBundle(null);
        setBundle(await invoke("load_configuration", { path }));
      }
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function importFile(preview: boolean) {
    setBusy(true);
    setMessage("");
    setReport(null);
    try {
      const value = await api<ImportReport>(
        `/configuration/${preview ? "preview" : "import"}`,
        "POST",
        request(),
      );
      setReport(value);
      if (!preview) {
        setBundle(null);
        setMessage(t("설정 가져오기를 완료했습니다."));
        await onChanged();
      }
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="settings-card">
      <h2>{t("설정과 채널 백업")}</h2>
      <p className="muted">
        {t(
          "채널, 녹화 규칙, 대기 예약과 공통 설정을 옮깁니다. 인증 파일, 백업 디스크와 원격 연결 설정은 현재 기기의 값을 사용합니다.",
        )}
      </p>
      <div className="tool-actions">
        <button
          type="button"
          className="button"
          disabled={busy}
          onClick={() => void exportFile()}
        >
          {t("설정 내보내기")}
        </button>
        <button
          type="button"
          className="button"
          disabled={busy}
          onClick={() => void load()}
        >
          {t("설정 파일 열기")}
        </button>
      </div>
      {bundle !== null && (
        <>
          <label className="field">
            {t("가져온 예약의 저장 폴더")}
            <input
              disabled={busy}
              value={storage}
              onChange={(e) => {
                setStorage(e.target.value);
                setReport(null);
              }}
            />
          </label>
          <label className="check-field">
            <input
              type="checkbox"
              disabled={busy}
              checked={replace}
              onChange={(e) => {
                setReplace(e.target.checked);
                setReport(null);
              }}
            />
            {t("같은 채널의 규칙과 옵션 교체")}
          </label>
          <label className="check-field">
            <input
              type="checkbox"
              disabled={busy}
              checked={settings}
              onChange={(e) => {
                setSettings(e.target.checked);
                setReport(null);
              }}
            />
            {t("공통 녹화 설정도 적용")}
          </label>
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => void importFile(true)}
          >
            {t("가져오기 미리보기")}
          </button>
          {report && (
            <button
              type="button"
              className="button primary"
              disabled={busy}
              onClick={() => void importFile(false)}
            >
              {t("가져오기 적용")}
            </button>
          )}
        </>
      )}
      {report && (
        <div role="status">
          <p>
            {t("채널 추가 / 교체 / 건너뜀")}: {report.added_channels} /{" "}
            {report.replaced_channels} / {report.skipped_channels}
          </p>
          <p>
            {t("예약 추가 / 중복 / 만료")}: {report.added_schedules} /{" "}
            {report.skipped_schedules} / {report.expired_schedules}
          </p>
          <p>
            {t("적용될 원본 정리 기간")}:{" "}
            {report.retention.cleanup_after_days ?? t("사용 안 함")}
          </p>
          <p>
            {t("적용될 전체 삭제 기간")}:{" "}
            {report.retention.delete_after_days ?? t("사용 안 함")}
          </p>
          <p>{report.storage_root}</p>
        </div>
      )}
      {message && <p role="status">{t(message)}</p>}
    </section>
  );
}
