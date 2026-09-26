import { useEffect, useRef, useState } from "react";
import { api, mediaUrl, queueDownload } from "./api";
import { useI18n } from "./i18n";
import {
  bytes,
  isTerminal,
  type Job,
  type LibraryFile,
  type Operation,
} from "./types";

export function MediaLibrary({
  job,
  operations,
  remote,
  onChanged,
}: {
  job: Job;
  operations: Operation[];
  remote: boolean;
  onChanged: () => Promise<void>;
}) {
  const { t } = useI18n();
  const video = useRef<HTMLVideoElement>(null);
  const [files, setFiles] = useState<LibraryFile[]>([]);
  const [source, setSource] = useState("");
  const [url, setUrl] = useState("");
  const [duration, setDuration] = useState(0);
  const [start, setStart] = useState(0);
  const [end, setEnd] = useState(0);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const completed = operations
    .filter((o) => o.request.job_id === job.id && o.state === "completed")
    .map((o) => o.id)
    .join(",");
  const preview = operations.find(
    (o) =>
      o.request.job_id === job.id &&
      o.request.task.kind === "preview" &&
      o.request.task.path === source,
  );
  const path =
    preview?.state === "completed"
      ? preview.result?.path?.split(/[\\/]/).pop()
      : undefined;
  useEffect(() => {
    let cancelled = false;
    void api<LibraryFile[]>(`/jobs/${job.id}/files`)
      .then((f) => {
        if (!cancelled) {
          setFiles(f);
          setSource((old) => old || f.find((x) => !x.preview)?.path || "");
        }
      })
      .catch((e) => {
        if (!cancelled) setMessage(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [job.id, completed]);
  useEffect(() => {
    let cancelled = false;
    setUrl("");
    setStart(0);
    setEnd(0);
    setDuration(0);
    if (path)
      void mediaUrl(job.id, path)
        .then((value) => {
          if (!cancelled) setUrl(value);
        })
        .catch((e) => {
          if (!cancelled) setMessage(String(e));
        });
    return () => {
      cancelled = true;
    };
  }, [job.id, path]);
  async function queue(task: unknown) {
    setBusy(true);
    setMessage("");
    try {
      await api("/operations", "POST", [{ job_id: job.id, task }]);
      setMessage(t("작업 목록에 추가했습니다."));
      await onChanged();
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="job-tool-section">
      <h4>{t("파일 미리보기와 다운로드")}</h4>
      <label className="field">
        {t("미리볼 파일")}
        <select
          aria-label={t("미리볼 파일")}
          value={source}
          onChange={(e) => {
            setSource(e.target.value);
            setUrl("");
          }}
        >
          <option value="">{t("선택")}</option>
          {files
            .filter((f) => !f.preview)
            .map((f) => (
              <option key={f.path} value={f.path}>
                {f.path} ({bytes(f.bytes)})
              </option>
            ))}
        </select>
      </label>
      <div className="tool-actions">
        <button
          className="button small"
          disabled={
            busy ||
            !source ||
            !isTerminal(job) ||
            (preview && ["queued", "running"].includes(preview.state))
          }
          onClick={() => void queue({ kind: "preview", path: source })}
        >
          {t("미리보기 파일 만들기")}
        </button>
        {path && (
          <button
            className="button small"
            onClick={() =>
              void mediaUrl(job.id, path)
                .then(setUrl)
                .catch((e) => setMessage(String(e)))
            }
          >
            {t("미리보기 다시 열기")}
          </button>
        )}
        {remote && (
          <button
            className="button small"
            disabled={busy || !source}
            onClick={async () => {
              setBusy(true);
              try {
                await queueDownload(job.id, source);
                setMessage(t("이 컴퓨터의 다운로드 작업에 추가했습니다."));
              } catch (e) {
                setMessage(String(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {t("이 컴퓨터로 다운로드")}
          </button>
        )}
      </div>
      <p className="muted">
        {t(
          "미리보기는 원본을 유지하고 별도 H.264/AAC 파일로 생성합니다. 진행 상태는 작업 화면에서 확인하세요.",
        )}
      </p>
      {url && (
        <>
          <video
            ref={video}
            controls
            preload="metadata"
            src={url}
            style={{ width: "100%", maxHeight: 360 }}
            onLoadedMetadata={() => {
              const d = video.current?.duration ?? 0;
              if (Number.isFinite(d)) {
                setDuration(d);
                setEnd(d);
              }
            }}
            onError={() =>
              setMessage(
                t("미리보기를 재생하지 못했습니다. 연결을 다시 열어 주세요."),
              )
            }
          />
          <div className="tool-actions">
            {(job.bookmarks ?? [])
              .filter(
                (b) =>
                  b.media_seconds !== null &&
                  source.includes(
                    `attempt-${String(b.attempt).padStart(4, "0")}/`,
                  ),
              )
              .map((b) => (
                <button
                  className="button small"
                  key={b.id}
                  onClick={() => {
                    if (video.current) {
                      video.current.currentTime = Math.min(
                        b.media_seconds!,
                        duration,
                      );
                      void video.current.play().catch(() => {});
                    }
                  }}
                >
                  {b.title}
                </button>
              ))}
          </div>
          <label className="field">
            {t("클립 시작 초")}
            <input
              type="number"
              min="0"
              max={duration}
              step="0.1"
              value={start}
              onChange={(e) => setStart(Number(e.target.value))}
            />
            <input
              aria-label={t("클립 시작 위치")}
              type="range"
              min="0"
              max={duration}
              step="0.1"
              value={start}
              onChange={(e) => setStart(Number(e.target.value))}
            />
          </label>
          <label className="field">
            {t("클립 끝 초")}
            <input
              type="number"
              min="0"
              max={duration}
              step="0.1"
              value={end}
              onChange={(e) => setEnd(Number(e.target.value))}
            />
            <input
              aria-label={t("클립 끝 위치")}
              type="range"
              min="0"
              max={duration}
              step="0.1"
              value={end}
              onChange={(e) => setEnd(Number(e.target.value))}
            />
          </label>
          <div className="tool-actions">
            <button
              className="button small"
              onClick={() => setStart(video.current?.currentTime ?? 0)}
            >
              {t("현재 위치를 시작으로")}
            </button>
            <button
              className="button small"
              onClick={() => setEnd(video.current?.currentTime ?? 0)}
            >
              {t("현재 위치를 끝으로")}
            </button>
            <button
              className="button"
              disabled={busy || end <= start || start < 0 || end > duration}
              onClick={() =>
                void queue({ kind: "range_clip", path: source, start, end })
              }
            >
              {t("선택 구간 내보내기")}
            </button>
          </div>
        </>
      )}
      {remote &&
        files
          .filter((f) => f.path !== source)
          .map((f) => (
            <div key={f.path} className="tool-actions">
              <span>{f.path}</span>
              <button
                className="button small"
                disabled={busy}
                onClick={async () => {
                  try {
                    await queueDownload(job.id, f.path);
                    setMessage(t("이 컴퓨터의 다운로드 작업에 추가했습니다."));
                  } catch (e) {
                    setMessage(String(e));
                  }
                }}
              >
                {t("다운로드")}
              </button>
            </div>
          ))}
      {message && <p role="status">{t(message)}</p>}
    </section>
  );
}
