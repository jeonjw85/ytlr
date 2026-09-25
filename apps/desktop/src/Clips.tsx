import { useState } from "react";
import { api } from "./api";
import { useI18n } from "./i18n";
import { isTerminal, type Job } from "./types";

export function Clips({
  job,
  onChanged,
}: {
  job: Job;
  onChanged: () => Promise<void>;
}) {
  const { t } = useI18n();
  const bookmarks = (job.bookmarks ?? []).filter(
    (b) => b.media_seconds !== null,
  );
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const [before, setBefore] = useState(30);
  const [after, setAfter] = useState(30);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState("");
  const chosen = bookmarks.find((b) => b.id === start);
  return (
    <section className="job-tool-section">
      <h4>{t("북마크 클립 내보내기")}</h4>
      <p className="muted">
        {t(
          "재인코딩 없이 추출하므로 키프레임에 따라 경계가 조금 달라질 수 있습니다. 같은 녹화 시도의 위치가 확인된 북마크만 사용할 수 있습니다.",
        )}
      </p>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          setResult("");
          try {
            const r = await api<{ path: string }>(
              `/jobs/${job.id}/clip`,
              "POST",
              {
                bookmark_id: start,
                end_bookmark_id: end || null,
                before_seconds: before,
                after_seconds: after,
              },
            );
            setResult(`${t("내보내기 완료")}: ${r.path}`);
            await onChanged();
          } catch (e) {
            setResult(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <label className="field">
          {t("시작 북마크")}
          <select
            aria-label={t("시작 북마크")}
            required
            value={start}
            onChange={(e) => {
              setStart(e.target.value);
              setEnd("");
            }}
          >
            <option value="">{t("선택")}</option>
            {bookmarks.map((b) => (
              <option key={b.id} value={b.id}>
                {b.title}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          {t("끝 북마크")}
          <select
            aria-label={t("끝 북마크")}
            value={end}
            onChange={(e) => setEnd(e.target.value)}
          >
            <option value="">{t("전후 구간 사용")}</option>
            {bookmarks
              .filter(
                (b) =>
                  b.attempt === chosen?.attempt &&
                  b.media_seconds! > chosen.media_seconds!,
              )
              .map((b) => (
                <option key={b.id} value={b.id}>
                  {b.title}
                </option>
              ))}
          </select>
        </label>
        {!end && (
          <div className="tool-actions">
            <label>
              {t("이전 초")}
              <input
                required
                type="number"
                min="0"
                value={before}
                onChange={(e) => setBefore(Number(e.target.value))}
              />
            </label>
            <label>
              {t("이후 초")}
              <input
                required
                type="number"
                min="0"
                value={after}
                onChange={(e) => setAfter(Number(e.target.value))}
              />
            </label>
          </div>
        )}
        <button
          className="button"
          disabled={busy || !isTerminal(job) || !chosen}
        >
          {t("클립 만들기")}
        </button>
      </form>
      {result && <p role="status">{t(result)}</p>}
    </section>
  );
}
