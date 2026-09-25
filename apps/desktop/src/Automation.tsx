import { useState } from "react";
import { api } from "./api";
import { useI18n } from "./i18n";
import { deadlineDraft } from "./JobTools";
import {
  defaultAutomation,
  type AutomationSettings,
  type ChannelRules,
  type Job,
  type NotificationTarget,
  type RecordingSchedule,
  type RuleDecision,
} from "./types";

export function StartFields({
  value,
  onChange,
}: {
  value: RecordingSchedule;
  onChange: (value: RecordingSchedule) => void;
}) {
  const { t } = useI18n();
  return (
    <>
      <label className="field">
        {t("시작 시각 (이 기기 시간대)")}
        <input
          type="datetime-local"
          value={deadlineDraft(value.start_at).at}
          onChange={(e) =>
            onChange({
              ...value,
              start_at: e.target.value
                ? new Date(e.target.value).toISOString()
                : null,
            })
          }
        />
        <small>{t("비워 두면 즉시 시작합니다.")}</small>
      </label>
      <label className="field">
        {t("실제 녹화 시작 후 최대 분")}
        <input
          type="number"
          min="1"
          max="525600"
          value={value.duration_minutes ?? ""}
          onChange={(e) =>
            onChange({
              ...value,
              duration_minutes: e.target.value ? Number(e.target.value) : null,
            })
          }
        />
      </label>
    </>
  );
}

export function StartSchedule({
  job,
  onChanged,
}: {
  job: Job;
  onChanged: () => Promise<void>;
}) {
  const { t, dateLocale } = useI18n();
  const [draft, setDraft] = useState<RecordingSchedule>(
    job.schedule ?? { start_at: null, duration_minutes: null },
  );
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <section className="job-tool-section">
      <h4>{t("시작 예약")}</h4>
      <p>
        {job.schedule?.start_at
          ? new Date(job.schedule.start_at).toLocaleString(dateLocale)
          : t("즉시 시작")}
        {job.schedule?.duration_minutes
          ? ` · ${job.schedule.duration_minutes} ${t("분")}`
          : ""}
      </p>
      {job.attempt === 0 &&
        ["queued", "waiting"].includes(job.state) &&
        !job.stop_requested && (
          <button
            className="button small"
            onClick={() => {
              setDraft(
                job.schedule ?? { start_at: null, duration_minutes: null },
              );
              setEditing(!editing);
            }}
          >
            {t("시작 예약 변경")}
          </button>
        )}
      {editing && (
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            setBusy(true);
            setError("");
            try {
              await api(`/jobs/${job.id}/start-schedule`, "PUT", draft);
              await onChanged();
              setEditing(false);
            } catch (e) {
              setError(String(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          <StartFields value={draft} onChange={setDraft} />
          <button className="button" disabled={busy}>
            {t("예약 저장")}
          </button>
        </form>
      )}
      {error && <p role="alert">{t(error)}</p>}
    </section>
  );
}

const time = (minute: number) =>
  `${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}`;
const minute = (value: string) => {
  const [h, m] = value.split(":").map(Number);
  return h * 60 + m;
};

export function RuleFields({
  value,
  onChange,
  decisions = [],
}: {
  value: ChannelRules;
  onChange: (rules: ChannelRules) => void;
  decisions?: RuleDecision[];
}) {
  const { t, dateLocale } = useI18n();
  const [title, setTitle] = useState("");
  const [at, setAt] = useState("");
  const [result, setResult] = useState("");
  const [busy, setBusy] = useState(false);
  const window = value.window;
  return (
    <section className="job-tool-section">
      <h4>{t("자동 녹화 규칙")}</h4>
      {(["include", "exclude"] as const).map((key) => (
        <label className="field" key={key}>
          {t(
            key === "include"
              ? "포함 키워드 (한 줄에 하나)"
              : "제외 키워드 (한 줄에 하나)",
          )}
          <textarea
            value={value[key].join("\n")}
            onChange={(e) =>
              onChange({ ...value, [key]: e.target.value.split("\n") })
            }
          />
        </label>
      ))}
      <p className="muted">
        {t(
          "포함 키워드는 하나 이상 일치하면 허용하며, 제외 키워드가 우선합니다. 빈 줄은 저장 시 제외합니다.",
        )}
      </p>
      <label className="check-field">
        <input
          type="checkbox"
          checked={!!window}
          onChange={(e) =>
            onChange({
              ...value,
              window: e.target.checked
                ? {
                    weekdays: [1, 2, 3, 4, 5, 6, 7],
                    start_minute: 0,
                    end_minute: 0,
                    utc_offset_minutes: -new Date().getTimezoneOffset(),
                  }
                : null,
            })
          }
        />
        {t("주간 반복 녹화 시간대")}
      </label>
      {window && (
        <>
          <div className="tool-actions">
            {["월", "화", "수", "목", "금", "토", "일"].map((day, i) => (
              <label key={day}>
                <input
                  type="checkbox"
                  checked={window.weekdays.includes(i + 1)}
                  onChange={(e) =>
                    onChange({
                      ...value,
                      window: {
                        ...window,
                        weekdays: e.target.checked
                          ? [...window.weekdays, i + 1]
                          : window.weekdays.filter((d) => d !== i + 1),
                      },
                    })
                  }
                />
                {t(day)}
              </label>
            ))}
          </div>
          <label className="field">
            {t("반복 시작")}
            <input
              required
              type="time"
              value={time(window.start_minute)}
              onChange={(e) =>
                onChange({
                  ...value,
                  window: { ...window, start_minute: minute(e.target.value) },
                })
              }
            />
          </label>
          <label className="field">
            {t("반복 종료")}
            <input
              required
              type="time"
              value={time(window.end_minute)}
              onChange={(e) =>
                onChange({
                  ...value,
                  window: { ...window, end_minute: minute(e.target.value) },
                })
              }
            />
          </label>
          <label className="field">
            {t("UTC 오프셋 (분)")}
            <input
              required
              type="number"
              min="-720"
              max="840"
              value={window.utc_offset_minutes}
              onChange={(e) =>
                onChange({
                  ...value,
                  window: {
                    ...window,
                    utc_offset_minutes: Number(e.target.value),
                  },
                })
              }
            />
            <small>
              {t(
                "한국은 540. 고정 오프셋이며 일광 절약 시간은 자동 변경하지 않습니다. 종료가 시작 이하이면 다음 날 종료합니다.",
              )}
            </small>
          </label>
        </>
      )}
      <label className="field">
        {t("실제 녹화 시작 후 최대 분")}
        <input
          type="number"
          min="1"
          max="525600"
          value={value.duration_minutes ?? ""}
          onChange={(e) =>
            onChange({
              ...value,
              duration_minutes: e.target.value ? Number(e.target.value) : null,
            })
          }
        />
      </label>
      <label className="field">
        {t("미리보기 방송 제목")}
        <input value={title} onChange={(e) => setTitle(e.target.value)} />
      </label>
      <label className="field">
        {t("미리보기 시각")}
        <input
          type="datetime-local"
          value={at}
          onChange={(e) => setAt(e.target.value)}
        />
      </label>
      <button
        type="button"
        className="button small"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          try {
            const d = await api<RuleDecision>("/rules/preview", "POST", {
              rules: cleanRules(value),
              title,
              at: at ? new Date(at).toISOString() : null,
            });
            setResult(
              `${t(d.allowed ? "녹화 허용" : "녹화 제외")} · ${t(d.reason)}`,
            );
          } catch (e) {
            setResult(t(String(e)));
          } finally {
            setBusy(false);
          }
        }}
      >
        {t("규칙 미리보기")}
      </button>
      {result && <p role="status">{result}</p>}
      {decisions.length > 0 && (
        <details>
          <summary>{t("최근 규칙 판단")}</summary>
          {[...decisions].reverse().map((d, i) => (
            <p key={i}>
              {new Date(d.at).toLocaleString(dateLocale)} · {d.title} ·{" "}
              {t(d.reason)}
            </p>
          ))}
        </details>
      )}
    </section>
  );
}
export const cleanRules = (rules: ChannelRules): ChannelRules => ({
  ...rules,
  include: rules.include.map((s) => s.trim()).filter(Boolean),
  exclude: rules.exclude.map((s) => s.trim()).filter(Boolean),
});

export function AutomationFields({
  value = defaultAutomation,
  onChange,
}: {
  value?: AutomationSettings;
  onChange: (value: AutomationSettings) => void;
}) {
  const { t } = useI18n();
  const [status, setStatus] = useState<unknown>(null);
  const [error, setError] = useState("");
  const updateTarget = (i: number, target: NotificationTarget) =>
    onChange({
      ...value,
      notifications: value.notifications.map((v, n) => (n === i ? target : v)),
    });
  return (
    <section className="settings-card">
      <h2>{t("자동화와 보관 정책")}</h2>
      <p className="muted">
        {t(
          "보관 기간은 녹화 종료일부터 계산합니다. 중요 녹화는 정리·삭제에서 제외됩니다. 비워 두면 자동 처리하지 않습니다.",
        )}
      </p>
      {(["cleanup_after_days", "delete_after_days"] as const).map((key) => (
        <label className="field" key={key}>
          {t(
            key === "cleanup_after_days"
              ? "검증된 분할 원본 정리 (일 후)"
              : "녹화 전체 삭제 (일 후)",
          )}
          <input
            type="number"
            min="1"
            max="36500"
            value={value.retention[key] ?? ""}
            onChange={(e) =>
              onChange({
                ...value,
                retention: {
                  ...value.retention,
                  [key]: e.target.value ? Number(e.target.value) : null,
                },
              })
            }
          />
        </label>
      ))}
      <h3>{t("서비스 외부 알림")}</h3>
      <p className="muted">
        {t(
          "URL·토큰 대신 서비스 프로세스에 설정한 환경변수 이름을 입력하세요. 앱을 닫아도 전송하며 실패 시 재시도합니다.",
        )}
      </p>
      {value.notifications.map((target, i) => (
        <fieldset key={i} className="job-tool-section">
          <legend>
            {t("알림 대상")} {i + 1}
          </legend>
          <label className="field">
            ID
            <input
              required
              value={target.id}
              onChange={(e) =>
                updateTarget(i, { ...target, id: e.target.value })
              }
            />
          </label>
          <label className="field">
            {t("전송 방식")}
            <select
              aria-label={t("전송 방식")}
              value={target.kind}
              onChange={(e) =>
                updateTarget(
                  i,
                  e.target.value === "telegram"
                    ? {
                        id: target.id,
                        kind: "telegram",
                        token_env: "YTLR_TELEGRAM_TOKEN",
                        chat_id: "",
                      }
                    : {
                        id: target.id,
                        kind: e.target.value as "webhook" | "discord",
                        url_env: "YTLR_WEBHOOK_URL",
                      },
                )
              }
            >
              <option value="webhook">Webhook</option>
              <option value="discord">Discord</option>
              <option value="telegram">Telegram</option>
            </select>
          </label>
          {target.kind === "telegram" ? (
            <>
              <label className="field">
                {t("토큰 환경변수")}
                <input
                  required
                  value={target.token_env}
                  onChange={(e) =>
                    updateTarget(i, { ...target, token_env: e.target.value })
                  }
                />
              </label>
              <label className="field">
                Chat ID
                <input
                  required
                  value={target.chat_id}
                  onChange={(e) =>
                    updateTarget(i, { ...target, chat_id: e.target.value })
                  }
                />
              </label>
            </>
          ) : (
            <label className="field">
              {t("URL 환경변수")}
              <input
                required
                value={target.url_env}
                onChange={(e) =>
                  updateTarget(i, { ...target, url_env: e.target.value })
                }
              />
            </label>
          )}
          <button
            type="button"
            className="button small"
            onClick={() =>
              onChange({
                ...value,
                notifications: value.notifications.filter((_, n) => n !== i),
              })
            }
          >
            {t("삭제")}
          </button>
        </fieldset>
      ))}
      <div className="tool-actions">
        <button
          type="button"
          className="button"
          disabled={value.notifications.length >= 10}
          onClick={() =>
            onChange({
              ...value,
              notifications: [
                ...value.notifications,
                {
                  id: crypto.randomUUID(),
                  kind: "webhook",
                  url_env: "YTLR_WEBHOOK_URL",
                },
              ],
            })
          }
        >
          {t("알림 대상 추가")}
        </button>
        {["notifications", "maintenance"].map((path) => (
          <button
            key={path}
            type="button"
            className="button"
            onClick={async () => {
              setError("");
              try {
                setStatus(await api(`/${path}`));
              } catch (e) {
                setError(String(e));
              }
            }}
          >
            {t(path === "notifications" ? "알림 전송 내역" : "자동 정리 내역")}
          </button>
        ))}
      </div>
      {status !== null && (
        <pre className="automation-log">{JSON.stringify(status, null, 2)}</pre>
      )}
      {error && <p role="alert">{t(error)}</p>}
    </section>
  );
}
