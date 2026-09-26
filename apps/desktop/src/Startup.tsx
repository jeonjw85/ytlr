import { useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { useI18n } from "./i18n";

export function Startup() {
  const { t } = useI18n();
  const [status, setStatus] = useState<{
    enabled: boolean;
    start_hidden: boolean;
  } | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (isTauri())
      void invoke<{ enabled: boolean; start_hidden: boolean }>("startup_status")
        .then(setStatus)
        .catch((e) => setError(String(e)));
  }, []);
  async function change(value: { enabled: boolean; start_hidden: boolean }) {
    const previous = status;
    setStatus(value);
    setBusy(true);
    setError("");
    try {
      setStatus(
        await invoke("set_startup", {
          enabled: value.enabled,
          startHidden: value.start_hidden,
        }),
      );
    } catch (e) {
      setStatus(previous);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  if (!isTauri()) return null;
  return (
    <section className="settings-card">
      <h2>{t("자동 시작")}</h2>
      {status && (
        <>
          <label className="check-field">
            <input
              type="checkbox"
              checked={status.enabled}
              disabled={busy}
              onChange={(e) =>
                void change({ ...status, enabled: e.target.checked })
              }
            />
            {t("로그인 시 녹화 앱 자동 시작")}
          </label>
          <label className="check-field">
            <input
              type="checkbox"
              checked={status.start_hidden}
              disabled={busy}
              onChange={(e) =>
                void change({ ...status, start_hidden: e.target.checked })
              }
            />
            {t("자동 시작 시 트레이로 시작")}
          </label>
        </>
      )}
      <p className="muted">
        {t(
          "로그인 후 로컬 녹화 서비스를 시작합니다. 컴퓨터가 꺼져 있거나 절전 중이면 예약을 실행할 수 없습니다.",
        )}
      </p>
      {error && <p role="alert">{t(error)}</p>}
    </section>
  );
}
