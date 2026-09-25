import { useState } from "react";
import { api, scopedApi } from "./api";
import { useI18n } from "./i18n";
import {
  bytes,
  isTerminal,
  stateLabels,
  type CleanupPlan,
  type Job,
} from "./types";

export interface LibraryFilter {
  search: string;
  channel: string;
  state: string;
  from: string;
  to: string;
  sort: string;
  protectedOnly: boolean;
}
export function filterLibrary(jobs: Job[], filter: LibraryFilter): Job[] {
  return jobs
    .filter(
      (j) =>
        isTerminal(j) &&
        `${j.title} ${j.channel}`
          .toLowerCase()
          .includes(filter.search.toLowerCase()) &&
        (!filter.channel || j.channel === filter.channel) &&
        (!filter.state || j.state === filter.state) &&
        (!filter.protectedOnly || j.protected) &&
        (!filter.from ||
          new Date(j.created_at).getTime() >=
            new Date(`${filter.from}T00:00:00`).getTime()) &&
        (!filter.to ||
          new Date(j.created_at).getTime() <
            new Date(`${filter.to}T23:59:59.999`).getTime() + 1),
    )
    .sort((a, b) =>
      filter.sort === "size"
        ? b.bytes - a.bytes
        : filter.sort === "title"
          ? a.title.localeCompare(b.title)
          : filter.sort === "oldest"
            ? Date.parse(a.created_at) - Date.parse(b.created_at)
            : Date.parse(b.created_at) - Date.parse(a.created_at),
    );
}

export function Library({
  jobs,
  onChanged,
  onOpen,
  onDetail,
  onDelete,
}: {
  jobs: Job[];
  onChanged: () => Promise<void>;
  onOpen: (job: Job) => void;
  onDetail: (id: string) => void;
  onDelete: (job: Job) => Promise<void>;
}) {
  const { t, dateLocale } = useI18n();
  const [filter, setFilter] = useState<LibraryFilter>({
    search: "",
    channel: "",
    state: "",
    from: "",
    to: "",
    sort: "newest",
    protectedOnly: false,
  });
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<string[]>([]);
  const [plans, setPlans] = useState<CleanupPlan[]>([]);
  const shown = filterLibrary(jobs, filter);
  const chosen = shown.filter((j) => selected.has(j.id));
  const change = (patch: Partial<LibraryFilter>) => {
    setFilter({ ...filter, ...patch });
    setPlans([]);
  };
  async function batch(kind: "export" | "preview" | "cleanup") {
    const api = scopedApi();
    setBusy(true);
    setResults([]);
    if (kind === "preview") setPlans([]);
    const items =
      kind === "cleanup"
        ? plans
            .map((p) => jobs.find((j) => j.id === p.job_id))
            .filter((j): j is Job => !!j)
        : chosen;
    const prepared: CleanupPlan[] = [];
    try {
      for (const job of items) {
        try {
          let message = "";
          if (kind === "preview") {
            const plan = await api<CleanupPlan>(
              `/jobs/${job.id}/cleanup/preview`,
              "POST",
              {},
            );
            prepared.push(plan);
            message = `${t("정리 가능")}: ${bytes(plan.reclaimable_bytes)}`;
          } else if (kind === "cleanup") {
            const plan = plans.find((p) => p.job_id === job.id)!;
            if (!plan.files.length) {
              message = t("정리할 파일 없음");
            } else {
              const r = await api<{
                completed: boolean;
                reclaimed_bytes: number;
              }>(`/jobs/${job.id}/cleanup`, "POST", {
                plan_id: plan.id,
                all: true,
              });
              message = `${t(r.completed ? "정리 완료" : "일부 정리")}: ${bytes(r.reclaimed_bytes)}`;
            }
          } else {
            if (!job.outputs.length)
              throw new Error(t("결과 파일이 없습니다."));
            let completed = 0;
            for (let index = 0; index < job.outputs.length; index++) {
              try {
                await api(`/jobs/${job.id}/export-wait`, "POST", { index });
                completed++;
              } catch (e) {
                throw new Error(
                  `${completed}/${job.outputs.length} · ${String(e)}`,
                );
              }
            }
            message = `${t("내보내기 완료")} (${completed})`;
          }
          setResults((r) => [...r, `${job.title}: ${message}`]);
        } catch (e) {
          setResults((r) => [
            ...r,
            `${job.title}: ${t("실패")} · ${String(e)}`,
          ]);
        }
      }
      if (kind === "preview") setPlans(prepared);
      if (kind === "cleanup") setPlans([]);
      await onChanged();
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <div className="section-bar">
        <h2>
          {t("보관된 작업")} <span>{shown.length}</span>
        </h2>
        <input
          className="search"
          aria-label={t("녹화 검색")}
          placeholder={t("제목 또는 채널 검색")}
          value={filter.search}
          disabled={busy}
          onChange={(e) => change({ search: e.target.value })}
        />
      </div>
      <fieldset disabled={busy} className="library-filters">
        <label>
          {t("채널")}
          <select
            aria-label={t("채널")}
            value={filter.channel}
            onChange={(e) => change({ channel: e.target.value })}
          >
            <option value="">{t("전체")}</option>
            {[...new Set(jobs.filter(isTerminal).map((j) => j.channel))]
              .filter(Boolean)
              .sort()
              .map((c) => (
                <option key={c}>{c}</option>
              ))}
          </select>
        </label>
        <label>
          {t("상태")}
          <select
            aria-label={t("상태")}
            value={filter.state}
            onChange={(e) => change({ state: e.target.value })}
          >
            <option value="">{t("전체")}</option>
            {(["completed", "partial", "stopped", "failed"] as const).map(
              (s) => (
                <option value={s} key={s}>
                  {t(stateLabels[s])}
                </option>
              ),
            )}
          </select>
        </label>
        <label>
          {t("시작 날짜")}
          <input
            type="date"
            value={filter.from}
            onChange={(e) => change({ from: e.target.value })}
          />
        </label>
        <label>
          {t("끝 날짜")}
          <input
            type="date"
            value={filter.to}
            onChange={(e) => change({ to: e.target.value })}
          />
        </label>
        <label>
          {t("정렬")}
          <select
            aria-label={t("정렬")}
            value={filter.sort}
            onChange={(e) => change({ sort: e.target.value })}
          >
            {[
              ["newest", "최신순"],
              ["oldest", "오래된순"],
              ["size", "용량순"],
              ["title", "제목순"],
            ].map(([v, label]) => (
              <option key={v} value={v}>
                {t(label)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <input
            type="checkbox"
            checked={filter.protectedOnly}
            onChange={(e) => change({ protectedOnly: e.target.checked })}
          />
          {t("중요 녹화만")}
        </label>
      </fieldset>
      <div className="tool-actions">
        <label>
          <input
            type="checkbox"
            disabled={busy}
            checked={shown.length > 0 && chosen.length === shown.length}
            onChange={(e) => {
              setSelected(
                e.target.checked ? new Set(shown.map((j) => j.id)) : new Set(),
              );
              setPlans([]);
            }}
          />
          {t("표시된 작업 전체 선택")} ({chosen.length})
        </label>
        <button
          className="button"
          disabled={busy || !chosen.length}
          onClick={() => void batch("export")}
        >
          {t("선택 내보내기")}
        </button>
        <button
          className="button"
          disabled={busy || !chosen.length}
          onClick={() => void batch("preview")}
        >
          {t("선택 정리 미리보기")}
        </button>
      </div>
      {plans.length > 0 && (
        <div className="job-tool-section">
          <h4>{t("정리 대상 파일")}</h4>
          {plans.map((p) => (
            <details key={p.id}>
              <summary>
                {jobs.find((j) => j.id === p.job_id)?.title} · {p.files.length}{" "}
                · {bytes(p.reclaimable_bytes)}
              </summary>
              {p.files.map((f) => (
                <p key={f.path}>
                  {f.path} · {bytes(f.bytes)}
                </p>
              ))}
            </details>
          ))}
          <button
            className="button danger"
            disabled={busy || !plans.some((p) => p.files.length)}
            onClick={() => {
              if (
                window.confirm(
                  t("미리보기에 표시된 분할 원본을 모두 정리할까요?"),
                )
              )
                void batch("cleanup");
            }}
          >
            {t("미리보기 파일 정리 실행")}
          </button>
        </div>
      )}
      {(busy || results.length > 0) && (
        <div role="status" className="job-tool-section">
          {busy && <p>{t("처리 중")}</p>}
          {results.map((r, i) => (
            <p key={i}>{r}</p>
          ))}
        </div>
      )}
      <div className="library-list">
        {shown.map((job) => (
          <div className="library-row" key={job.id}>
            <input
              type="checkbox"
              aria-label={`${job.title} ${t("선택")}`}
              disabled={busy}
              checked={selected.has(job.id)}
              onChange={(e) => {
                const next = new Set(selected);
                if (e.target.checked) next.add(job.id);
                else next.delete(job.id);
                setSelected(next);
                setPlans([]);
              }}
            />
            <div className="library-title">
              <h3>
                {job.title === "방송 정보 확인 대기" ? t(job.title) : job.title}
              </h3>
              <p>
                {job.channel || job.video_id} ·{" "}
                {new Date(job.created_at).toLocaleDateString(dateLocale)} ·{" "}
                {bytes(job.bytes)} ·{" "}
                {t(
                  job.recording_options?.audio_only ? "음성만" : "영상 + 음성",
                )}
                {!job.recording_options?.audio_only && (
                  <>
                    {" "}
                    ·{" "}
                    {job.recording_options?.max_height
                      ? `${job.recording_options.max_height}p`
                      : t("최고 화질")}
                  </>
                )}
              </p>
              <span className="library-note">{t(job.message)}</span>
            </div>
            <span>{t(stateLabels[job.state])}</span>
            <button
              className="button small"
              disabled={busy}
              aria-pressed={job.protected ?? false}
              onClick={async () => {
                try {
                  await api(`/jobs/${job.id}/protect`, "PUT", {
                    protected: !job.protected,
                  });
                  await onChanged();
                } catch (e) {
                  setResults([String(e)]);
                }
              }}
            >
              {t(job.protected ? "보호 해제" : "중요 녹화 보호")}
            </button>
            <button className="button small" onClick={() => onOpen(job)}>
              {t("폴더 열기")}
            </button>
            <button
              className="button small danger"
              disabled={busy || job.protected}
              onClick={() => void onDelete(job)}
            >
              {t("작업 삭제")}
            </button>
            <button className="button small" onClick={() => onDetail(job.id)}>
              {t("상세")}
            </button>
          </div>
        ))}
      </div>
      {!shown.length && <p>{t("검색 결과가 없습니다")}</p>}
    </>
  );
}
