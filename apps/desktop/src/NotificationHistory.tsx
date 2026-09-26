import { useState } from "react";
import { api, scopedApi } from "./api";
import { useI18n } from "./i18n";

interface Delivery {
  id: number;
  target: string;
  attempts: number;
  next_at: string;
  delivered: boolean;
  error: string | null;
  body: { kind: string; message: string; at: string };
}
interface TargetStatus {
  id: string;
  kind: string;
  configured: boolean;
}
export function NotificationHistory() {
  const { t, dateLocale } = useI18n();
  const [rows, setRows] = useState<Delivery[]>([]);
  const [targets, setTargets] = useState<TargetStatus[]>([]);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function refresh() {
    const request = scopedApi();
    setBusy(true);
    setError("");
    try {
      const [r, s] = await Promise.all([
        request<Delivery[]>("/notifications"),
        request<TargetStatus[]>("/notifications/targets"),
      ]);
      setRows(r);
      setTargets(s);
      setOpen(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function send(path: string, body: unknown = {}) {
    setBusy(true);
    setError("");
    try {
      await api(path, "POST", body);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="job-tool-section">
      <button
        type="button"
        className="button"
        disabled={busy}
        onClick={() => void refresh()}
      >
        {t("알림 연결과 전송 내역")}
      </button>
      {open && (
        <>
          <p className="muted">
            {t(
              "저장된 알림 설정으로 테스트합니다. 전송 결과는 새로고침해서 확인하세요.",
            )}
          </p>
          {targets.map((target) => (
            <div className="tool-actions" key={target.id}>
              <span>
                {target.id} ({target.kind}) ·{" "}
                {t(target.configured ? "환경변수 확인됨" : "환경변수 미설정")}
              </span>
              <button
                type="button"
                className="button small"
                disabled={busy}
                onClick={() =>
                  void send("/notifications/test", { target_id: target.id })
                }
              >
                {t("테스트 알림 보내기")}
              </button>
            </div>
          ))}
          {rows.length === 0 && <p>{t("전송 내역이 없습니다.")}</p>}
          {rows.map((row) => (
            <div key={row.id} className="job-tool-section">
              <strong>
                {row.target} · {t(row.delivered ? "전송 완료" : "전송 대기")}
              </strong>
              <p>{t(row.body.message)}</p>
              <p className="muted">
                {t("전송 시도")}: {row.attempts} ·{" "}
                {new Date(row.body.at).toLocaleString(dateLocale)}
              </p>
              {row.error && <p className="inline-error">{t(row.error)}</p>}
              {!row.delivered && (
                <>
                  <p className="muted">
                    {t("다음 전송")}:{" "}
                    {new Date(row.next_at).toLocaleString(dateLocale)}
                  </p>
                  <button
                    type="button"
                    className="button small"
                    disabled={busy}
                    onClick={() => void send(`/notifications/${row.id}/retry`)}
                  >
                    {t("지금 재전송")}
                  </button>
                </>
              )}
            </div>
          ))}
        </>
      )}
      {error && <p role="alert">{t(error)}</p>}
    </div>
  );
}
