import { useEffect, useState } from "react";
import { api, openJob } from "./api";
import { useI18n } from "./i18n";
import {
  bytes,
  duration,
  isTerminal,
  type Job,
  type Bookmark,
  type CleanupPlan,
} from "./types";

export interface DeadlineDraft {
  mode: "none" | "minutes" | "at";
  minutes: string;
  at: string;
}

export function deadlineDraft(stopAt?: string | null): DeadlineDraft {
  const date = stopAt ? new Date(stopAt) : null;
  return {
    mode: date ? "at" : "none",
    minutes: "120",
    at: date
      ? new Date(date.getTime() - date.getTimezoneOffset() * 60000)
          .toISOString()
          .slice(0, 16)
      : "",
  };
}

export function resolveDeadline(draft: DeadlineDraft): string | null {
  if (draft.mode === "none") return null;
  const minutes = Number(draft.minutes);
  if (
    draft.mode === "minutes" &&
    (!Number.isInteger(minutes) || minutes < 1 || minutes > 525600)
  )
    throw new Error("종료까지의 시간은 1~525600분이어야 합니다.");
  const time =
    draft.mode === "minutes"
      ? Date.now() + minutes * 60000
      : new Date(draft.at).getTime();
  if (!Number.isFinite(time) || time <= Date.now())
    throw new Error("미래의 종료 시각을 입력하세요.");
  return new Date(time).toISOString();
}

export function DeadlineFields({
  value,
  onChange,
}: {
  value: DeadlineDraft;
  onChange: (value: DeadlineDraft) => void;
}) {
  const { t } = useI18n();
  return (
    <>
      <label className="field">
        {t("종료 예약")}
        <select
          aria-label={t("종료 예약")}
          value={value.mode}
          onChange={(e) =>
            onChange({
              ...value,
              mode: e.target.value as DeadlineDraft["mode"],
            })
          }
        >
          <option value="none">{t("예약 없음")}</option>
          <option value="minutes">{t("지금부터 N분 뒤")}</option>
          <option value="at">{t("종료 시각 지정")}</option>
        </select>
      </label>
      {value.mode === "minutes" && (
        <label className="field">
          {t("종료까지 남은 분")}
          <input
            type="number"
            required
            min="1"
            max="525600"
            value={value.minutes}
            onChange={(e) => onChange({ ...value, minutes: e.target.value })}
          />
          <small>{t("방송 시작 대기 시간도 포함됩니다.")}</small>
        </label>
      )}
      {value.mode === "at" && (
        <label className="field">
          {t("종료 시각 (이 기기 시간대)")}
          <input
            type="datetime-local"
            required
            value={value.at}
            onChange={(e) => onChange({ ...value, at: e.target.value })}
          />
        </label>
      )}
    </>
  );
}

export function JobSchedule({
  job,
  onChanged,
}: {
  job: Job;
  onChanged: () => Promise<void>;
}) {
  const { t, dateLocale } = useI18n();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(() => deadlineDraft(job.stop_at));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <section className="job-tool-section">
      <h4>{t("종료 예약")}</h4>
      <p>
        {job.stop_at
          ? new Date(job.stop_at).toLocaleString(dateLocale)
          : t("예약 없음")}
      </p>
      {!isTerminal(job) && !job.stop_requested && (
        <button
          className="button small"
          onClick={() => {
            setDraft(deadlineDraft(job.stop_at));
            setEditing(!editing);
          }}
        >
          {t("종료 예약 변경")}
        </button>
      )}
      {editing && !isTerminal(job) && !job.stop_requested && (
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            setBusy(true);
            setError("");
            try {
              await api(`/jobs/${job.id}/schedule`, "PUT", {
                stop_at: resolveDeadline(draft),
              });
              await onChanged();
              setEditing(false);
            } catch (error) {
              setError(String(error));
            } finally {
              setBusy(false);
            }
          }}
        >
          <DeadlineFields value={draft} onChange={setDraft} />
          <button className="button small" disabled={busy}>
            {t("예약 저장")}
          </button>
        </form>
      )}
      {error && (
        <p className="inline-error" role="alert">
          {t(error)}
        </p>
      )}
    </section>
  );
}

function BookmarkRow({
  job,
  bookmark,
  onChanged,
}: {
  job: Job;
  bookmark: Bookmark;
  onChanged: () => Promise<void>;
}) {
  const { t, dateLocale } = useI18n();
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(bookmark.title);
  const [note, setNote] = useState(bookmark.note);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const index = job.outputs.findIndex((o) =>
    o.path
      .split(/[\\/]/)
      .includes(`attempt-${String(bookmark.attempt).padStart(4, "0")}`),
  );
  async function change(remove: boolean) {
    setBusy(true);
    setError("");
    try {
      await api(
        `/jobs/${job.id}/bookmarks/${bookmark.id}`,
        remove ? "DELETE" : "PUT",
        remove ? undefined : { title, note },
      );
      await onChanged();
      setEditing(false);
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="bookmark-row">
      <strong>{bookmark.title}</strong>
      <p className="muted">
        {t("시도 {attempt}", { attempt: bookmark.attempt })} ·{" "}
        {bookmark.media_seconds === null
          ? t("미디어 시점 미확인")
          : `${duration(bookmark.media_seconds)} (${t("근사 미디어 시점")})`}{" "}
        · {new Date(bookmark.created_at).toLocaleString(dateLocale)}
      </p>
      {bookmark.note && <p className="bookmark-note">{bookmark.note}</p>}
      <div className="tool-actions">
        {index >= 0 && (
          <button
            className="button small"
            onClick={() =>
              void openJob(job.id, index).catch((e) => setError(String(e)))
            }
          >
            {t("해당 구간 파일 열기")}
          </button>
        )}
        <button
          className="button small"
          disabled={busy}
          onClick={() => {
            setTitle(bookmark.title);
            setNote(bookmark.note);
            setEditing(!editing);
          }}
        >
          {t("북마크 편집")}
        </button>
        <button
          className="button small danger"
          disabled={busy}
          onClick={() => {
            if (window.confirm(t("이 북마크를 삭제할까요?"))) void change(true);
          }}
        >
          {t("북마크 삭제")}
        </button>
      </div>
      {editing && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void change(false);
          }}
        >
          <label className="field">
            {t("북마크 제목")}
            <input
              required
              maxLength={120}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
            />
          </label>
          <label className="field">
            {t("북마크 메모")}
            <textarea
              maxLength={2000}
              value={note}
              onChange={(e) => setNote(e.target.value)}
            />
          </label>
          <button className="button small" disabled={busy}>
            {t("북마크 저장")}
          </button>
        </form>
      )}
      {error && (
        <p className="inline-error" role="alert">
          {t(error)}
        </p>
      )}
    </div>
  );
}

export function JobBookmarks({
  job,
  onChanged,
}: {
  job: Job;
  onChanged: () => Promise<void>;
}) {
  const { t } = useI18n();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <section className="job-tool-section">
      <h4>{t("북마크")}</h4>
      <p className="muted">
        {t(
          "수신 시점의 기록입니다. 구간 파일을 연 뒤 표시된 시간을 참고하세요.",
        )}
      </p>
      {job.state === "recording" && !job.stop_requested && (
        <button
          className="button small"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError("");
            try {
              await api(`/jobs/${job.id}/bookmarks`, "POST", {
                title: t("북마크 {count}", {
                  count: (job.bookmarks?.length ?? 0) + 1,
                }),
              });
              await onChanged();
            } catch (error) {
              setError(String(error));
            } finally {
              setBusy(false);
            }
          }}
        >
          {t("지금 표시")}
        </button>
      )}
      {(job.bookmarks ?? []).map((bookmark) => (
        <BookmarkRow
          key={bookmark.id}
          job={job}
          bookmark={bookmark}
          onChanged={onChanged}
        />
      ))}
      {!job.bookmarks?.length && (
        <p className="muted">{t("저장된 북마크가 없습니다.")}</p>
      )}
      {error && (
        <p className="inline-error" role="alert">
          {t(error)}
        </p>
      )}
    </section>
  );
}

const categoryLabels: Record<string, string> = {
  results: "검증된 결과",
  exports: "내보낸 파일",
  fragments: "원본 조각",
  segments: "분할 세그먼트",
  sources: "수집 원본",
  metadata_other: "메타데이터 및 기타",
};

export function JobCleanup({
  job,
  onChanged,
}: {
  job: Job;
  onChanged: () => Promise<void>;
}) {
  const { t } = useI18n();
  const [sizes, setSizes] = useState<Record<string, number> | null>(null);
  const [plan, setPlan] = useState<CleanupPlan | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<{
    reclaimed_bytes: number;
    pending_files: string[];
    completed: boolean;
    warnings: string[];
  } | null>(null);
  useEffect(() => {
    let live = true;
    void api<Record<string, number>>(`/jobs/${job.id}/storage`)
      .then((sizes) => {
        if (live) setSizes(sizes);
      })
      .catch((error) => {
        if (live) setError(String(error));
      });
    return () => {
      live = false;
    };
  }, [job.id]);
  return (
    <section className="job-tool-section">
      <h4>{t("파일 용량 및 정리")}</h4>
      <dl className="detail-grid">
        {Object.entries(sizes ?? {}).map(([kind, size]) => (
          <div key={kind}>
            <dt>{t(categoryLabels[kind] ?? kind)}</dt>
            <dd>{bytes(size)}</dd>
          </div>
        ))}
      </dl>
      <p className="muted">
        {t(
          "검증된 병합 결과로 대체된 완성 세그먼트만 정리합니다. 수집 원본·불완전 조각·결과·내보낸 파일은 보존됩니다.",
        )}
      </p>
      <button
        className="button small"
        disabled={
          busy ||
          !["completed", "stopped"].includes(job.state) ||
          job.continuity_uncertain ||
          !job.outputs.length
        }
        onClick={async () => {
          setBusy(true);
          setError("");
          setResult(null);
          setPlan(null);
          try {
            const next = await api<CleanupPlan>(
              `/jobs/${job.id}/cleanup/preview`,
              "POST",
              {},
            );
            setPlan(next);
            setSelected(new Set(next.files.map((f) => f.path)));
          } catch (error) {
            setError(String(error));
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? t("파일 검증·처리 중…") : t("정리 미리보기")}
      </button>
      {plan && (
        <>
          <p>
            {t("보존할 결과")} ·{" "}
            {plan.retained.map((o) => o.path.split(/[\\/]/).pop()).join(", ")}
          </p>
          <p>
            {t("선택한 삭제 용량")} ·{" "}
            {bytes(
              plan.files
                .filter((f) => selected.has(f.path))
                .reduce((n, f) => n + f.bytes, 0),
            )}
          </p>
          {!plan.files.length && (
            <p>{t("정리할 수 있는 세그먼트가 없습니다.")}</p>
          )}
          <div className="cleanup-files">
            {plan.files.map((file) => (
              <label className="check-field" key={file.path}>
                <input
                  type="checkbox"
                  disabled={busy}
                  checked={selected.has(file.path)}
                  onChange={(e) =>
                    setSelected((previous) => {
                      const next = new Set(previous);
                      if (e.target.checked) next.add(file.path);
                      else next.delete(file.path);
                      return next;
                    })
                  }
                />
                <span>
                  {file.path} · {bytes(file.bytes)}
                </span>
              </label>
            ))}
          </div>
          {!!plan.files.length && (
            <button
              className="button small danger"
              disabled={busy || !selected.size}
              onClick={async () => {
                if (
                  !window.confirm(
                    t(
                      "선택한 세그먼트를 영구 삭제할까요? 검증된 결과 파일은 보존됩니다.",
                    ),
                  )
                )
                  return;
                setBusy(true);
                setError("");
                try {
                  const cleanup = await api<{
                    reclaimed_bytes: number;
                    pending_files: string[];
                    completed: boolean;
                    warnings: string[];
                  }>(`/jobs/${job.id}/cleanup`, "POST", {
                    plan_id: plan.id,
                    files: [...selected],
                  });
                  setPlan(null);
                  setResult(cleanup);
                  setSizes(await api(`/jobs/${job.id}/storage`));
                  await onChanged();
                } catch (error) {
                  setError(String(error));
                } finally {
                  setBusy(false);
                }
              }}
            >
              {t("선택 파일 영구 삭제")}
            </button>
          )}
        </>
      )}
      {error && (
        <p className="inline-error" role="alert">
          {t(error)}
        </p>
      )}
      {result && (
        <div
          className={result.completed ? "inline-success" : "inline-error"}
          role={result.completed ? "status" : "alert"}
        >
          <p>
            {t(result.completed ? "정리 완료 · 회수 용량 {size}" : "정리 일부 완료 · 회수 용량 {size}", {
              size: bytes(result.reclaimed_bytes),
            })}
          </p>
          {!!result.pending_files.length && (
            <>
              <p>{t("삭제 대기 파일 · 다음 정리 때 다시 처리합니다.")}</p>
              <ul>
                {result.pending_files.map((file) => (
                  <li key={file}>{file}</li>
                ))}
              </ul>
            </>
          )}
          {!!result.warnings.length && (
            <ul>
              {result.warnings.map((warning) => (
                <li key={warning}>{t(warning)}</li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}
