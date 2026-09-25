import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import {
  Activity,
  AlertCircle,
  ArrowDownToLine,
  ArrowUpRight,
  Check,
  CheckCircle2,
  Circle,
  Clock3,
  Download,
  FileVideo2,
  FolderOpen,
  HardDrive,
  LayoutDashboard,
  Loader2,
  Plus,
  Radio,
  RefreshCw,
  Search,
  Settings2,
  ShieldCheck,
  Square,
  Terminal,
  Trash2,
  WifiOff,
  X,
} from "lucide-react";
import { isTauri, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { api, connectRemote, listRemotes, openJob, saveRemote } from "./api";
import { useI18n, type Language } from "./i18n";
import {
  AutomationFields,
  RuleFields,
  StartFields,
  StartSchedule,
  cleanRules,
} from "./Automation";
import { Library } from "./Library";
import { Clips } from "./Clips";
import { AppUpdates } from "./AppUpdates";
import {
  DeadlineFields,
  deadlineDraft,
  resolveDeadline,
  JobSchedule,
  JobBookmarks,
  JobCleanup,
} from "./JobTools";
import {
  bytes,
  defaultRecordingOptions,
  defaultRules,
  type ChannelRules,
  type RecordingSchedule,
  duration,
  elapsed,
  isTerminal,
  stateLabels,
  type Job,
  type Channel,
  type RecordingOptions,
  type JobEvent,
  type Remote,
  type Settings,
  type Snapshot,
} from "./types";

type Page = "recordings" | "channels" | "library" | "settings";
const pageInfo: Record<Page, string> = {
  recordings: "녹화",
  channels: "채널",
  library: "보관함",
  settings: "설정",
};

export default function App() {
  const { t, dateLocale } = useI18n();
  const [page, setPage] = useState<Page>("recordings");
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [connectionError, setConnectionError] = useState("");
  const [toast, setToast] = useState<{
    message: string;
    error: boolean;
  } | null>(null);
  const [add, setAdd] = useState<"record" | "channel" | "remote" | null>(null);
  const [remotes, setRemotes] = useState<Remote[]>([]);
  const [connection, setConnection] = useState("local");
  const [detail, setDetail] = useState<string | null>(null);
  const [editingChannel, setEditingChannel] = useState<Channel | null>(null);
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [clock, setClock] = useState(Date.now());
  const previous = useRef<Map<string, string>>(new Map());
  const warnedStorage = useRef<Set<string>>(new Set());
  const previousAlerts = useRef<Map<string, boolean>>(new Map());
  const initialized = useRef(false);
  const fetching = useRef(false);
  const refreshPending = useRef(false);
  const connectionEpoch = useRef(0);

  const notify = useCallback(
    (message: string, error = false) => setToast({ message, error }),
    [],
  );
  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    if (fetching.current) {
      refreshPending.current = true;
      return;
    }
    let repeat = true;
    while (repeat) {
      refreshPending.current = false;
      fetching.current = true;
      const epoch = connectionEpoch.current;
      try {
        const next = await api<Snapshot>("/snapshot");
        if (epoch !== connectionEpoch.current) return;
        setSnapshot(next);
        setConnectionError("");
        const lowStorage = (next.storage ?? []).filter(
          (disk) => disk.low_space,
        );
        if (next.settings.notifications) {
          for (const disk of lowStorage) {
            if (!warnedStorage.current.has(disk.path)) {
              const granted = await isPermissionGranted();
              if (epoch !== connectionEpoch.current) return;
              if (granted) {
                sendNotification({
                  title: `${t("저장 공간 부족 경고")} · YTLR`,
                  body: `${disk.path} · ${bytes(disk.free_bytes)}`,
                });
              }
            }
          }
        }
        if (epoch !== connectionEpoch.current) return;
        warnedStorage.current = new Set(lowStorage.map((disk) => disk.path));
        for (const job of next.jobs) {
          for (const alert of job.alerts ?? []) {
            const before = previousAlerts.current.get(alert.id);
            const changed =
              next.settings.notifications &&
              ((alert.active && before !== true) ||
                (!alert.active && before === true));
            if (changed) {
              const granted = await isPermissionGranted();
              if (epoch !== connectionEpoch.current) return;
              if (granted) {
                sendNotification({
                  title: `${t(alert.active ? "녹화 이상 알림" : "녹화 알림 해제")} · YTLR`,
                  body: `${job.title} · ${t(alert.message)}`,
                });
              }
            }
          }
        }
        if (epoch !== connectionEpoch.current) return;
        previousAlerts.current = new Map(
          next.jobs.flatMap((j) =>
            (j.alerts ?? []).map((a) => [a.id, a.active] as const),
          ),
        );
        if (initialized.current && next.settings.notifications) {
          for (const job of next.jobs) {
            if (
              previous.current.get(job.id) !== job.state &&
              ["completed", "partial"].includes(job.state)
            ) {
              const granted = await isPermissionGranted();
              if (epoch !== connectionEpoch.current) return;
              if (granted)
                sendNotification({
                  title: `${t(stateLabels[job.state])} · YTLR`,
                  body: job.title,
                });
            }
          }
        }
        if (epoch !== connectionEpoch.current) return;
        previous.current = new Map(next.jobs.map((j) => [j.id, j.state]));
        initialized.current = true;
      } catch (error) {
        if (epoch === connectionEpoch.current)
          setConnectionError(String(error));
      } finally {
        if (epoch === connectionEpoch.current) fetching.current = false;
      }
      repeat = epoch === connectionEpoch.current && refreshPending.current;
    }
  }, [t]);

  useEffect(() => {
    if (!isTauri()) return;
    void listRemotes()
      .then(setRemotes)
      .catch((e) => notify(String(e), true));
  }, [notify]);
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => {
      setClock(Date.now());
      void refresh();
    }, 2000);
    return () => clearInterval(timer);
  }, [refresh]);
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 6500);
    return () => clearTimeout(timer);
  }, [toast]);

  async function switchConnection(name: string) {
    connectionEpoch.current++;
    fetching.current = false;
    refreshPending.current = false;
    initialized.current = false;
    warnedStorage.current.clear();
    previousAlerts.current.clear();
    setSnapshot(null);
    setDetail(null);
    setEditingChannel(null);
    setAdd(null);
    setConnection(name);
    setBusy(true);
    try {
      await connectRemote(name === "local" ? null : name);
      setConnectionError("");
      await refresh();
    } catch (error) {
      setConnectionError(String(error));
      notify(String(error), true);
    } finally {
      setBusy(false);
    }
  }

  async function action(
    path: string,
    method = "POST",
    body: unknown = {},
    success?: string,
  ) {
    if (connectionError)
      throw new Error(t("연결 상태를 확인한 뒤 다시 시도하세요."));
    setBusy(true);
    try {
      await api(path, method, body);
      if (success) notify(success);
      await refresh();
    } catch (e) {
      notify(String(e), true);
      throw e;
    } finally {
      setBusy(false);
    }
  }
  const safeAction = (
    path: string,
    method = "POST",
    body: unknown = {},
    success?: string,
  ) => {
    void action(path, method, body, success).catch(() => {});
  };
  const safeOpen = (job: Job, index?: number) => {
    void openJob(job.id, index).catch((e) => notify(String(e), true));
  };
  const deleteJob = async (job: Job) => {
    if (!window.confirm(t("이 작업과 저장된 녹화 파일을 모두 삭제할까요?"))) {
      return;
    }
    try {
      await action(
        `/jobs/${job.id}`,
        "DELETE",
        {},
        "작업과 녹화 파일을 삭제했습니다.",
      );
      if (detail === job.id) setDetail(null);
    } catch {
      // action already displays the error.
    }
  };
  const jobs = snapshot?.jobs ?? [];
  const running = jobs.filter((j) => !isTerminal(j));
  const recordingCount = jobs.filter((j) => j.state === "recording").length;
  const shown = jobs.filter(
    (j) =>
      (page === "library" ? isTerminal(j) : !isTerminal(j)) &&
      `${j.title} ${j.channel}`.toLowerCase().includes(search.toLowerCase()),
  );
  const selectedJob = jobs.find((j) => j.id === detail);
  const missingTools = snapshot?.tools.some((t) => t.error);
  const storage = snapshot?.storage?.find(
    (disk) => disk.path === snapshot.settings.storage_root,
  );

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">YTLR</div>
        <nav aria-label={t("주 메뉴")}>
          {(
            [
              ["recordings", LayoutDashboard, "녹화"],
              ["channels", Radio, "채널"],
              ["library", FolderOpen, "보관함"],
            ] as const
          ).map(([key, Icon, label]) => (
            <button
              key={key}
              className={`nav-item ${page === key ? "selected" : ""}`}
              onClick={() => {
                setPage(key);
                setSearch("");
              }}
            >
              <Icon size={19} />
              <span>{t(label)}</span>
              {key === "recordings" && running.length > 0 && (
                <span className="nav-count">{running.length}</span>
              )}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="local-status">
            <span
              className={`status-light ${connectionError || !snapshot ? "offline" : ""}`}
            />
            <div>
              {t("녹화 장비")}
              <small>
                {t(
                  connectionError
                    ? "연결 확인 필요"
                    : snapshot
                      ? "연결됨"
                      : "서비스 연결 중",
                )}
                {snapshot && !connectionError && ` · v${snapshot.version}`}
              </small>
              <select
                aria-label={t("연결 대상")}
                value={connection}
                disabled={busy}
                onChange={(e) => void switchConnection(e.target.value)}
              >
                <option value="local">{t("이 기기")}</option>
                {remotes.map((remote) => (
                  <option key={remote.id} value={remote.name}>
                    {remote.name}
                  </option>
                ))}
              </select>
              <button
                className="button ghost small"
                onClick={() => setAdd("remote")}
              >
                {t("원격 추가")}
              </button>
            </div>
          </div>
          <button
            className={`nav-item ${page === "settings" ? "selected" : ""}`}
            onClick={() => setPage("settings")}
          >
            <Settings2 size={19} />
            <span>{t("설정")}</span>
          </button>
        </div>
      </aside>

      <main>
        <header className="topbar">
          <div className="breadcrumb">{t(pageInfo[page])}</div>
          <div className="topbar-right">
            <span>{connection === "local" ? t("이 기기") : connection}</span>
          </div>
        </header>
        <div className="content">
          <AppUpdates settings={page === "settings"} />
          <div className="page-heading">
            <div>
              <h1>{t(pageInfo[page])}</h1>
            </div>
            {page !== "settings" && (
              <button
                className="button primary"
                onClick={() =>
                  setAdd(page === "channels" ? "channel" : "record")
                }
                disabled={!snapshot || !!connectionError || busy}
              >
                <Plus size={17} />
                {t(page === "channels" ? "채널 추가" : "새 녹화")}
              </button>
            )}
          </div>

          {!isTauri() && (
            <div className="notice">
              <Terminal size={20} />
              <div>
                <strong>{t("데스크톱 앱에서 실행하세요")}</strong>
                <p>
                  <code>pnpm dev</code> —{" "}
                  {t("프로젝트 루트에서 실행하면 녹화 서비스에 연결됩니다.")}
                </p>
              </div>
            </div>
          )}
          {connectionError && (
            <div className="notice danger">
              <WifiOff size={20} />
              <div>
                <strong>{t("서비스에 연결할 수 없습니다")}</strong>
                <p>
                  {t(
                    "녹화 상태를 확인할 수 없습니다. 마지막으로 확인한 정보를 표시합니다.",
                  )}
                </p>
                <small>{t(connectionError)}</small>
              </div>
              <button
                className="button small"
                onClick={() =>
                  connection === "local"
                    ? void refresh()
                    : void switchConnection(connection)
                }
              >
                <RefreshCw size={14} />
                {t("다시 연결")}
              </button>
            </div>
          )}
          {missingTools && page !== "settings" && (
            <div className="notice">
              <Download size={20} />
              <div>
                <strong>{t("녹화 엔진이 없습니다")}</strong>
                <p>
                  {t("설정에서 엔진을 설치하면 녹화를 시작할 수 있습니다.")}
                </p>
              </div>
              <button
                className="button small"
                onClick={() => setPage("settings")}
              >
                {t("엔진 설정")}
                <ArrowUpRight size={14} />
              </button>
            </div>
          )}

          {(snapshot?.storage ?? [])
            .filter((disk) => disk.low_space)
            .map((disk) => (
              <div className="notice danger" role="alert" key={disk.path}>
                <AlertCircle size={20} />
                <div>
                  <strong>{t("저장 공간 부족 경고")}</strong>
                  <p>
                    {disk.path} · {t("남은 저장 공간")} {bytes(disk.free_bytes)}
                  </p>
                  <p>
                    {t(
                      "공간이 부족하면 녹화가 중단됩니다. 저장 공간을 확보하세요.",
                    )}
                  </p>
                </div>
              </div>
            ))}
          {jobs.flatMap((job) =>
            (job.alerts ?? [])
              .filter((alert) => alert.active)
              .map((alert) => (
                <div className="notice danger" role="alert" key={alert.id}>
                  <AlertCircle size={20} />
                  <div>
                    <strong>{job.title}</strong>
                    <p>{t(alert.message)}</p>
                  </div>
                  <button
                    className="button small"
                    onClick={() => setDetail(job.id)}
                  >
                    {t("상세")}
                  </button>
                </div>
              )),
          )}
          {page === "recordings" && (
            <>
              <div className="stats">
                <Stat
                  label={t("지금 녹화 중")}
                  value={`${recordingCount}`}
                  suffix={`/ ${snapshot?.settings.max_recordings ?? 2}`}
                  detail={
                    running.length > recordingCount
                      ? t("{count}개 작업 대기·처리 중", {
                          count: running.length - recordingCount,
                        })
                      : recordingCount > 0
                        ? t("녹화 중")
                        : t("대기 중")
                  }
                />
                <Stat
                  label={t("녹화 자료")}
                  value={bytes(jobs.reduce((n, j) => n + j.bytes, 0))}
                  detail={t("원본·조각·결과")}
                />
                <Stat
                  label={t("남은 저장 공간")}
                  value={t(bytes(snapshot?.free_bytes ?? null))}
                  detail={
                    storage?.remaining_seconds != null
                      ? t("예약 공간 도달까지 약 {time}", {
                          time: duration(storage.remaining_seconds),
                        })
                      : t("저장 폴더가 있는 디스크")
                  }
                />
              </div>
              {storage && (
                <p className="tip">
                  {t("전체 용량")} {t(bytes(storage.total_bytes))} ·{" "}
                  {t("디스크 기록 속도")}{" "}
                  {storage.bytes_per_second == null
                    ? "—"
                    : `${bytes(storage.bytes_per_second)}/s`}
                  {" · "}
                  {t(
                    "남은 시간은 최근 전체 녹화 기록 속도 기준 추정치입니다. 병합·복구에는 추가 공간이 필요합니다.",
                  )}
                </p>
              )}
              <div className="section-bar">
                <h2>
                  {t("진행 중인 작업")} <span>{running.length}</span>
                </h2>
                <SearchBox value={search} onChange={setSearch} />
              </div>
              {shown.length ? (
                <div className="job-list">
                  {shown.map((job) => (
                    <JobCard
                      key={job.id}
                      job={job}
                      clock={clock}
                      onDetail={() => setDetail(job.id)}
                      onFolder={() => safeOpen(job)}
                      onStop={() => safeAction(`/jobs/${job.id}/stop`)}
                      onBookmark={() =>
                        safeAction(
                          `/jobs/${job.id}/bookmarks`,
                          "POST",
                          {
                            title: t("북마크 {count}", {
                              count: (job.bookmarks?.length ?? 0) + 1,
                            }),
                          },
                          "북마크를 저장했습니다.",
                        )
                      }
                      busy={busy}
                    />
                  ))}
                </div>
              ) : (
                <Empty
                  title={t(
                    search
                      ? "검색 결과가 없습니다"
                      : "진행 중인 녹화가 없습니다",
                  )}
                  text={t("라이브 URL을 추가하거나 채널을 등록하세요.")}
                  action={
                    <button
                      className="button"
                      disabled={!snapshot}
                      onClick={() => setAdd("record")}
                    >
                      <Plus size={16} />
                      {t("URL 추가")}
                    </button>
                  }
                />
              )}
              <p className="tip">{t("창을 닫아도 녹화는 계속됩니다.")}</p>
            </>
          )}

          {page === "channels" && (
            <>
              <div className="section-bar">
                <h2>
                  {t("등록한 채널")}{" "}
                  <span>{snapshot?.channels.length ?? 0}</span>
                </h2>
                <span className="muted">
                  {t("{seconds}초 간격으로 확인", {
                    seconds: snapshot?.settings.scan_interval_secs ?? 60,
                  })}
                </span>
              </div>
              {snapshot?.channels.length ? (
                <div className="channel-grid">
                  {snapshot.channels.map((channel) => (
                    <div className="channel-card" key={channel.id}>
                      <div className="channel-header">
                        <div className="channel-avatar">
                          {(
                            channel.name.replace(
                              /^https:\/\/www.youtube.com\/@?/,
                              "",
                            ) || "Y"
                          )
                            .slice(0, 1)
                            .toUpperCase()}
                        </div>
                        <button
                          className={`toggle ${channel.enabled ? "on" : ""}`}
                          role="switch"
                          aria-checked={channel.enabled}
                          aria-label={t("{name} 자동 녹화", {
                            name: channel.name,
                          })}
                          onClick={() =>
                            safeAction(`/channels/${channel.id}`, "PUT", {
                              ...channel,
                              enabled: !channel.enabled,
                            })
                          }
                        >
                          <span />
                        </button>
                      </div>
                      <h3>{channel.name}</h3>
                      <p className="channel-url">
                        {channel.url.replace("https://www.youtube.com/", "")}
                      </p>
                      <div className="channel-meta">
                        <span
                          className={`status-light ${channel.enabled ? "" : "offline"}`}
                        />
                        {t(
                          channel.enabled ? "방송 자동 감시" : "감시 일시 중지",
                        )}
                      </div>
                      <p>
                        <RecordingSummary options={channel.recording_options} />
                      </p>
                      <button
                        className="button small"
                        disabled={busy}
                        onClick={() => setEditingChannel(channel)}
                      >
                        <Settings2 size={14} />
                        {t("녹화 옵션")}
                      </button>
                      {channel.last_error && (
                        <p className="inline-error">{t(channel.last_error)}</p>
                      )}
                      <div className="channel-footer">
                        <span>
                          {channel.last_checked_at
                            ? t("마지막 확인 {time}", {
                                time: new Date(
                                  channel.last_checked_at,
                                ).toLocaleTimeString(dateLocale),
                              })
                            : t("첫 확인 대기")}
                        </span>
                        <button
                          className="icon-button"
                          aria-label={t("{name} 삭제", { name: channel.name })}
                          onClick={() =>
                            safeAction(`/channels/${channel.id}`, "DELETE")
                          }
                        >
                          <Trash2 size={15} />
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              ) : (
                <Empty
                  title={t("등록된 채널이 없습니다")}
                  text={t(
                    "채널을 등록하면 방송이 시작될 때 자동으로 녹화합니다.",
                  )}
                  action={
                    <button
                      className="button"
                      disabled={!snapshot}
                      onClick={() => setAdd("channel")}
                    >
                      <Plus size={16} />
                      {t("채널 추가")}
                    </button>
                  }
                />
              )}
            </>
          )}

          {page === "library" && (
            <Library
              key={connection}
              jobs={jobs}
              onChanged={refresh}
              onOpen={safeOpen}
              onDetail={setDetail}
              onDelete={deleteJob}
            />
          )}

          {page === "settings" && <AppPreferences />}
          {page === "settings" && snapshot && (
            <SettingsView
              snapshot={snapshot}
              remoteMode={connection !== "local"}
              busy={busy}
              onAction={action}
              onNotify={notify}
            />
          )}
          {!snapshot && isTauri() && !connectionError && (
            <div className="loading">
              <Loader2 className="spin" size={26} />
              <p>{t("녹화 서비스를 준비하고 있습니다…")}</p>
            </div>
          )}
        </div>
      </main>
      {add === "remote" && (
        <RemoteDialog
          onClose={() => setAdd(null)}
          onSave={async (remote) => {
            setRemotes(await saveRemote(remote));
            setAdd(null);
            notify(
              "원격을 저장했습니다. SSH 키 로그인이 설정되어 있어야 합니다.",
            );
          }}
        />
      )}
      {add && add !== "remote" && (
        <AddDialog
          kind={add}
          fromStart={snapshot?.settings.live_from_start ?? false}
          onClose={() => setAdd(null)}
          onSubmit={async (path, body) => {
            await action(path, "POST", body);
            setAdd(null);
            notify(
              add === "channel"
                ? "채널 감시를 시작합니다."
                : "녹화 작업을 추가했습니다.",
            );
          }}
        />
      )}
      {selectedJob && (
        <Details
          job={selectedJob}
          onClose={() => setDetail(null)}
          onAction={safeAction}
          onOpen={safeOpen}
          onNotify={notify}
          onChanged={refresh}
        />
      )}
      {editingChannel && (
        <ChannelOptionsDialog
          channel={editingChannel}
          onClose={() => setEditingChannel(null)}
          onSave={async (options, fromStart, rules) => {
            const current = snapshot?.channels.find(
              (channel) => channel.id === editingChannel.id,
            );
            if (!current) throw new Error(t("채널을 찾을 수 없습니다."));
            await action(`/channels/${current.id}`, "PUT", {
              ...current,
              recording_options: options,
              live_from_start: fromStart,
              rules,
            });
            setEditingChannel(null);
          }}
        />
      )}
      {toast && (
        <div
          className={`toast ${toast.error ? "error" : ""}`}
          role={toast.error ? "alert" : "status"}
        >
          {toast.error ? <AlertCircle size={18} /> : <CheckCircle2 size={18} />}
          <span>{t(toast.message)}</span>
          <button
            className="icon-button"
            aria-label={t("알림 닫기")}
            onClick={() => setToast(null)}
          >
            <X size={15} />
          </button>
        </div>
      )}
    </div>
  );
}

function Stat({
  label,
  value,
  suffix,
  detail,
}: {
  label: string;
  value: string;
  suffix?: string;
  detail: string;
}) {
  return (
    <div className="stat">
      <div className="stat-top">{label}</div>
      <div className="stat-value">
        {value}
        <small>{suffix}</small>
      </div>
      <div className="stat-detail">{detail}</div>
    </div>
  );
}
function SearchBox({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const { t } = useI18n();
  return (
    <label className="search">
      <Search size={15} />
      <input
        aria-label={t("녹화 검색")}
        placeholder={t("제목 또는 채널 검색")}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}
function Empty({
  title,
  text,
  action,
}: {
  title: string;
  text: string;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <h2>{title}</h2>
      <p>{text}</p>
      {action}
    </div>
  );
}
function Badge({ job }: { job: Job }) {
  const { t } = useI18n();
  return (
    <span className={`badge ${job.state}`}>
      {job.state === "recording" ? (
        <span className="live-dot" />
      ) : job.state === "completed" || job.state === "stopped" ? (
        <Check size={12} />
      ) : job.state === "reconnecting" ? (
        <RefreshCw size={12} />
      ) : (
        <Circle size={9} />
      )}{" "}
      {t(stateLabels[job.state])}
    </span>
  );
}
function JobCard({
  job,
  clock,
  onDetail,
  onFolder,
  onStop,
  onBookmark,
  busy,
}: {
  job: Job;
  clock: number;
  onDetail: () => void;
  onFolder: () => void;
  onStop: () => void;
  onBookmark: () => void;
  busy: boolean;
}) {
  const { t } = useI18n();
  return (
    <article className="job-card">
      <div className="job-top">
        <div className="job-title">
          <div className="job-channel">
            {job.channel || "YouTube"}
            <Badge job={job} />
          </div>
          <h3>
            {job.title === "방송 정보 확인 대기" ? t(job.title) : job.title}
          </h3>
          <p>
            <RecordingSummary options={job.recording_options} /> ·{" "}
            {job.resolution || job.format || job.url}
          </p>
        </div>
        <button
          className="icon-button"
          title={t("폴더 열기")}
          aria-label={t("폴더 열기")}
          onClick={onFolder}
        >
          <FolderOpen size={19} />
        </button>
      </div>
      <div className="job-metrics">
        <div>
          <Clock3 size={15} />
          <span>{t("경과 시간")}</span>
          <strong>{elapsed(job, clock)}</strong>
        </div>
        <div>
          <HardDrive size={15} />
          <span>{t("저장 자료")}</span>
          <strong>{bytes(job.bytes)}</strong>
        </div>
        <div>
          <Activity size={15} />
          <span>{t("최근 수신")}</span>
          <strong>
            {job.last_media_at
              ? t("{seconds}초 전", {
                  seconds: Math.max(
                    0,
                    Math.floor(
                      (clock - new Date(job.last_media_at).getTime()) / 1000,
                    ),
                  ),
                })
              : t("대기")}
          </strong>
        </div>
      </div>
      <div className="job-bottom">
        <span className={job.continuity_uncertain ? "warning-text" : "muted"}>
          {job.continuity_uncertain ? (
            <AlertCircle size={14} />
          ) : (
            <ShieldCheck size={14} />
          )}{" "}
          {t(job.message)}
        </span>
        <div>
          {job.state === "recording" && (
            <button
              className="button ghost small"
              disabled={busy || job.stop_requested}
              onClick={onBookmark}
            >
              {t("지금 표시")}
            </button>
          )}
          <button className="button ghost small" onClick={onDetail}>
            {t("상세")}
          </button>
          <button
            className="button small stop"
            disabled={busy || job.stop_requested}
            onClick={onStop}
          >
            <Square size={11} fill="currentColor" />
            {t(job.stop_requested ? "중지 중" : "중지")}
          </button>
        </div>
      </div>
      {job.schedule?.start_at && job.attempt === 0 && (
        <p className="job-message">
          {t("예약 시작까지 {time}", {
            time: duration(
              (new Date(job.schedule.start_at).getTime() - clock) / 1000,
            ),
          })}
        </p>
      )}
      {job.stop_at && (
        <p className="tip">
          {t("예약 종료까지 {time}", {
            time: duration((new Date(job.stop_at).getTime() - clock) / 1000),
          })}
        </p>
      )}
    </article>
  );
}

function Modal({
  title,
  children,
  onClose,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const panel = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    panel.current?.querySelector<HTMLElement>("input,button")?.focus();
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") close.current();
      if (e.key === "Tab") {
        const elements = Array.from(
          panel.current?.querySelectorAll<HTMLElement>(
            'button:not(:disabled),input:not(:disabled),select,textarea,[tabindex="0"]',
          ) ?? [],
        );
        const first = elements[0],
          last = elements[elements.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last?.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first?.focus();
        }
      }
    };
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("keydown", key);
      previous?.focus();
    };
  }, []);
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="modal"
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <div className="modal-heading">
          <h2>{title}</h2>
          <button
            className="icon-button"
            aria-label={t("닫기")}
            onClick={onClose}
          >
            <X size={20} />
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

function RecordingSummary({
  options = defaultRecordingOptions,
}: {
  options?: RecordingOptions;
}) {
  const { t } = useI18n();
  return (
    <>
      {options.audio_only
        ? t("음성만")
        : options.max_height
          ? t("최대 {height}p", { height: options.max_height })
          : t("최고 화질")}
    </>
  );
}

function RecordingFields({
  options,
  onChange,
}: {
  options: RecordingOptions;
  onChange: (options: RecordingOptions) => void;
}) {
  const { t } = useI18n();
  return (
    <>
      <label className="field">
        {t("녹화 모드")}
        <select
          value={options.audio_only ? "audio" : "video"}
          onChange={(e) =>
            onChange({
              audio_only: e.target.value === "audio",
              max_height: null,
            })
          }
        >
          <option value="video">{t("영상 + 음성")}</option>
          <option value="audio">{t("음성만")}</option>
        </select>
      </label>
      {!options.audio_only && (
        <label className="field">
          {t("최대 화질")}
          <select
            value={options.max_height ?? "best"}
            onChange={(e) =>
              onChange({
                ...options,
                max_height:
                  e.target.value === "best"
                    ? null
                    : (Number(
                        e.target.value,
                      ) as RecordingOptions["max_height"]),
              })
            }
          >
            <option value="best">{t("최고 화질")}</option>
            {[1080, 720, 480].map((height) => (
              <option value={height} key={height}>
                {height}p
              </option>
            ))}
          </select>
          <small>
            {t("선택한 해상도 이하에서 가장 좋은 스트림을 저장합니다.")}
          </small>
        </label>
      )}
      {options.audio_only && (
        <p className="muted">
          {t("음성 전용 스트림을 원본 코덱으로 저장합니다.")}
        </p>
      )}
    </>
  );
}

function ChannelOptionsDialog({
  channel,
  onClose,
  onSave,
}: {
  channel: Channel;
  onClose: () => void;
  onSave: (
    options: RecordingOptions,
    fromStart: boolean | null,
    rules: ChannelRules,
  ) => Promise<void>;
}) {
  const { t } = useI18n();
  const [options, setOptions] = useState(
    channel.recording_options ?? defaultRecordingOptions,
  );
  const [fromStart, setFromStart] = useState(channel.live_from_start ?? null);
  const [rules, setRules] = useState(channel.rules ?? defaultRules);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  return (
    <Modal title={t("채널 녹화 옵션")} onClose={onClose}>
      <form
        onSubmit={async (event) => {
          event.preventDefault();
          setPending(true);
          setError("");
          try {
            await onSave(options, fromStart, cleanRules(rules));
          } catch (error) {
            setError(String(error));
          } finally {
            setPending(false);
          }
        }}
      >
        <p className="modal-intro">
          {channel.name} · {t("변경 사항은 새로 생성되는 녹화에 적용됩니다.")}
        </p>
        <RecordingFields options={options} onChange={setOptions} />
        <RuleFields
          value={rules}
          onChange={setRules}
          decisions={channel.decisions}
        />
        <label className="field">
          {t("녹화 시작 지점")}
          <select
            value={fromStart === null ? "default" : String(fromStart)}
            onChange={(e) =>
              setFromStart(
                e.target.value === "default" ? null : e.target.value === "true",
              )
            }
          >
            <option value="default">{t("전체 설정 따르기")}</option>
            <option value="false">{t("현재 시점부터")}</option>
            <option value="true">{t("가능하면 방송 처음부터 저장")}</option>
          </select>
        </label>
        {error && (
          <p className="inline-error" role="alert">
            {t(error)}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="button" onClick={onClose}>
            {t("취소")}
          </button>
          <button className="button primary" disabled={pending}>
            {t("저장")}
          </button>
        </div>
      </form>
    </Modal>
  );
}

function AddDialog({
  kind,
  fromStart,
  onClose,
  onSubmit,
}: {
  kind: "record" | "channel";
  fromStart: boolean;
  onClose: () => void;
  onSubmit: (path: string, body: unknown) => Promise<void>;
}) {
  const { t } = useI18n();
  const [url, setUrl] = useState("");
  const [name, setName] = useState("");
  const [start, setStart] = useState(fromStart);
  const [options, setOptions] = useState(defaultRecordingOptions);
  const [deadline, setDeadline] = useState(() => deadlineDraft());
  const [schedule, setSchedule] = useState<RecordingSchedule>({
    start_at: null,
    duration_minutes: null,
  });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    setPending(true);
    setError("");
    try {
      await onSubmit(
        kind === "record" ? "/jobs" : "/channels",
        kind === "record"
          ? {
              url,
              live_from_start: start,
              priority: 0,
              recording_options: options,
              stop_at: resolveDeadline(deadline),
              schedule,
            }
          : {
              url,
              name,
              live_from_start: start,
              priority: 0,
              recording_options: options,
            },
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setPending(false);
    }
  }
  return (
    <Modal
      title={t(kind === "record" ? "새 라이브 녹화" : "채널 추가")}
      onClose={onClose}
    >
      <form onSubmit={submit}>
        <p className="modal-intro">
          {t(
            kind === "record"
              ? "진행 중인 라이브 또는 예약 방송의 URL을 입력하세요."
              : "등록한 채널의 새 라이브를 자동으로 녹화합니다.",
          )}
        </p>
        <label className="field">
          {t(kind === "record" ? "영상 URL" : "채널 URL")}
          <input
            required
            type="url"
            placeholder={
              kind === "record"
                ? "https://www.youtube.com/watch?v=…"
                : "https://www.youtube.com/@channel"
            }
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
        </label>
        <RecordingFields options={options} onChange={setOptions} />
        {kind === "record" && (
          <>
            <StartFields value={schedule} onChange={setSchedule} />
            <DeadlineFields value={deadline} onChange={setDeadline} />
          </>
        )}
        {kind === "channel" && (
          <label className="field">
            {t("표시 이름")} <span className="muted">{t("선택")}</span>
            <input
              placeholder={t("알아보기 쉬운 채널 이름")}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </label>
        )}
        <label className="check-field">
          <input
            type="checkbox"
            checked={start}
            onChange={(e) => setStart(e.target.checked)}
          />
          <div>
            {t("가능하면 방송 처음부터 저장")}
            <small>
              {t("유튜브에서 제공하는 과거 구간 범위에 따라 달라집니다.")}
            </small>
          </div>
        </label>
        {error && (
          <p className="inline-error" role="alert">
            {t(error)}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="button" onClick={onClose}>
            {t("취소")}
          </button>
          <button className="button primary" disabled={pending}>
            {pending ? (
              <Loader2 className="spin" size={16} />
            ) : (
              <Plus size={16} />
            )}
            {t("추가하기")}
          </button>
        </div>
      </form>
    </Modal>
  );
}

function RemoteDialog({
  onClose,
  onSave,
}: {
  onClose: () => void;
  onSave: (remote: Remote) => Promise<void>;
}) {
  const { t } = useI18n();
  const [name, setName] = useState("");
  const [ssh, setSsh] = useState("");
  const [dataDir, setDataDir] = useState("");
  const [identity, setIdentity] = useState("");
  const [serverExecutable, setServerExecutable] = useState("");
  const [knownHosts, setKnownHosts] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    setPending(true);
    setError("");
    try {
      await onSave({
        id: name.trim(),
        name: name.trim(),
        ssh: ssh.trim(),
        remote_data_dir: dataDir.trim(),
        identity: identity.trim() || null,
        executable: serverExecutable.trim() || null,
        known_hosts: knownHosts.trim() || null,
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setPending(false);
    }
  }
  return (
    <Modal title={t("SSH 원격 추가")} onClose={onClose}>
      <form onSubmit={submit}>
        <p className="modal-intro">
          {t(
            "서버에서 녹화 서비스가 localhost로만 열립니다. 이 앱은 SSH 터널로 연결합니다. 비밀번호는 저장하지 않으므로 키 로그인이 필요합니다.",
          )}
        </p>
        <label className="field">
          {t("이름")}
          <input
            required
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="studio"
          />
        </label>
        <label className="field">
          SSH
          <input
            required
            value={ssh}
            onChange={(e) => setSsh(e.target.value)}
            placeholder="ytlr@192.168.1.10"
          />
        </label>
        <label className="field">
          {t("서버 데이터 폴더")}
          <input
            required
            value={dataDir}
            onChange={(e) => setDataDir(e.target.value)}
            placeholder="/home/ytlr/recordings"
          />
        </label>
        <label className="field">
          {t("SSH 키")} <span className="muted">{t("선택")}</span>
          <input
            value={identity}
            onChange={(e) => setIdentity(e.target.value)}
            placeholder={t("비우면 ssh 기본 키")}
          />
        </label>
        <label className="field">
          {t("서버 ytlr 실행 파일 · 선택")}
          <input
            value={serverExecutable}
            onChange={(e) => setServerExecutable(e.target.value)}
            placeholder={t("/home/user/.local/bin/ytlr · 비우면 ytlr")}
          />
        </label>
        <label className="field">
          {t("known_hosts 파일 · 선택")}
          <input
            value={knownHosts}
            onChange={(e) => setKnownHosts(e.target.value)}
            placeholder={t("비우면 SSH 기본 호스트 키 저장소")}
          />
        </label>
        {error && (
          <p className="inline-error" role="alert">
            {t(error)}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="button" onClick={onClose}>
            {t("취소")}
          </button>
          <button className="button primary" disabled={pending}>
            {pending ? <Loader2 className="spin" size={16} /> : t("저장")}
          </button>
        </div>
      </form>
    </Modal>
  );
}

function Details({
  job,
  onClose,
  onAction,
  onOpen,
  onNotify,
  onChanged,
}: {
  job: Job;
  onClose: () => void;
  onAction: (
    path: string,
    method?: string,
    body?: unknown,
    success?: string,
  ) => void;
  onOpen: (job: Job, index?: number) => void;
  onNotify: (message: string, error?: boolean) => void;
  onChanged: () => Promise<void>;
}) {
  const { t, dateLocale } = useI18n();
  const [events, setEvents] = useState<JobEvent[]>([]);
  useEffect(() => {
    let live = true;
    void api<JobEvent[]>(`/jobs/${job.id}/events`)
      .then((events) => {
        if (live) setEvents(events);
      })
      .catch((e) => onNotify(String(e), true));
    return () => {
      live = false;
    };
  }, [job.id, job.updated_at, onNotify]);
  async function exportLog() {
    try {
      const path = await save({
        defaultPath: `ytlr-${job.video_id}-events.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (path) {
        await invoke("save_diagnostics", { id: job.id, path });
        onNotify("진단 로그를 저장했습니다.");
      }
    } catch (e) {
      onNotify(String(e), true);
    }
  }
  return (
    <Modal title={t("녹화 상세")} onClose={onClose}>
      <div className="detail-title">
        <Badge job={job} />
        <h3>
          {job.title === "방송 정보 확인 대기" ? t(job.title) : job.title}
        </h3>
        <p>{t(job.message)}</p>
      </div>
      <dl className="detail-grid">
        <div>
          <dt>{t("녹화 옵션")}</dt>
          <dd>
            <RecordingSummary options={job.recording_options} />
          </dd>
        </div>
        <div>
          <dt>{t("선택된 스트림")}</dt>
          <dd>{job.resolution || job.format || "—"}</dd>
        </div>
        <div>
          <dt>{t("수집 시도")}</dt>
          <dd>{t("{count}회", { count: job.attempt })}</dd>
        </div>
        <div>
          <dt>{t("재시도")}</dt>
          <dd>{t("{count}회", { count: job.retries })}</dd>
        </div>
        <div>
          <dt>{t("자료 용량")}</dt>
          <dd>{bytes(job.bytes)}</dd>
        </div>
        <div>
          <dt>{t("연속성")}</dt>
          <dd>
            {t(job.continuity_uncertain ? "확인 필요" : "확인된 누락 없음")}
          </dd>
        </div>
      </dl>
      {(job.gaps ?? []).length > 0 && (
        <>
          <h4>{t("추정 수신 공백 · 접합 경계 미검증")}</h4>
          {(job.gaps ?? []).map((gap) => (
            <p
              className="library-note"
              key={`${gap.after_attempt}-${gap.started_at}`}
            >
              {t("시도 {attempt} 이후 {duration}", {
                attempt: gap.after_attempt,
                duration: duration(gap.seconds),
              })}{" "}
              ·{" "}
              {t(
                gap.status === "recovered"
                  ? "복구됨"
                  : gap.status === "collected"
                    ? "보충 파일 확보 · 경계 미검증"
                    : gap.status === "unrecoverable"
                      ? "복구 불가"
                      : "미복구",
              )}
            </p>
          ))}
        </>
      )}
      <div className="path-box">
        {job.output_dir}
        <button
          className="icon-button"
          title={t("폴더 열기")}
          onClick={() => onOpen(job)}
        >
          <FolderOpen size={17} />
        </button>
      </div>
      {job.backup?.checked_at && (
        <p className={job.backup.state === "failed" ? "inline-error" : "muted"}>
          {t("백업")}{" "}
          {t(
            job.backup.state === "verified"
              ? "검증 완료"
              : job.backup.state === "failed"
                ? "재시도 필요"
                : "진행 중",
          )}{" "}
          · {t("{count}개 파일", { count: job.backup.verified_files })} ·{" "}
          {t(job.backup.message)}
        </p>
      )}
      {job.replica && (
        <p className="muted">
          {t("이중 녹화")} · {job.replica.target} · {job.replica.state} ·{" "}
          {t(job.replica.message)}
        </p>
      )}
      <JobSchedule key={`schedule-${job.id}`} job={job} onChanged={onChanged} />
      <StartSchedule key={`start-${job.id}`} job={job} onChanged={onChanged} />
      <Clips key={`clips-${job.id}`} job={job} onChanged={onChanged} />
      <JobBookmarks
        key={`bookmarks-${job.id}`}
        job={job}
        onChanged={onChanged}
      />
      {isTerminal(job) && (
        <JobCleanup key={`cleanup-${job.id}`} job={job} onChanged={onChanged} />
      )}
      {!!job.alerts?.length && (
        <section className="job-tool-section">
          <h4>{t("상태 알림")}</h4>
          {job.alerts.map((alert) => (
            <p
              key={alert.id}
              className={alert.active ? "warning-text" : "muted"}
            >
              {t(alert.message)} · {t(alert.active ? "확인 필요" : "알림 해제")}
            </p>
          ))}
        </section>
      )}
      <h4>{t("재생 가능한 결과")}</h4>
      {job.outputs.length ? (
        job.outputs.map((output, i) => (
          <div className="output-row" key={output.path}>
            <FileVideo2 size={18} />
            <div>
              <strong>{output.path.split(/[\\/]/).pop()}</strong>
              <small>
                {bytes(output.bytes)} ·{" "}
                {t(
                  job.recording_options?.audio_only
                    ? "음성 헤더 검사"
                    : "영상·음성 헤더 검사",
                )}
              </small>
            </div>
            <button className="button small" onClick={() => onOpen(job, i)}>
              {t("열기")}
            </button>
            <button
              className="icon-button"
              title={t(
                job.recording_options?.audio_only
                  ? "음성 내보내기 (MKA)"
                  : "MP4 내보내기",
              )}
              aria-label={t(
                job.recording_options?.audio_only
                  ? "음성 내보내기 (MKA)"
                  : "MP4 내보내기",
              )}
              onClick={() =>
                onAction(
                  `/jobs/${job.id}/export`,
                  "POST",
                  { index: i },
                  "내보내기를 시작했습니다. 진행 결과는 이벤트에 표시됩니다.",
                )
              }
            >
              <ArrowDownToLine size={17} />
            </button>
          </div>
        ))
      ) : (
        <p className="muted">
          {t(
            "아직 검증된 결과 파일이 없습니다. 수집 원본은 녹화 폴더에 보존됩니다.",
          )}
        </p>
      )}
      <div className="section-bar compact">
        <h4>{t("이벤트")}</h4>
        <button className="button ghost small" onClick={() => void exportLog()}>
          <Download size={14} />
          {t("로그 저장")}
        </button>
      </div>
      <div className="events">
        {events.length ? (
          events.map((e) => (
            <div className="event" key={e.id}>
              <time>{new Date(e.at).toLocaleTimeString(dateLocale)}</time>
              <span>{t(e.message)}</span>
            </div>
          ))
        ) : (
          <p className="muted">{t("기록된 이벤트가 없습니다.")}</p>
        )}
      </div>
      {isTerminal(job) && (
        <div className="modal-actions">
          <button
            className="button"
            disabled={!job.attempt}
            onClick={() => onAction(`/jobs/${job.id}/recover`)}
          >
            <ShieldCheck size={15} />
            {t("원본 복구")}
          </button>
          <button
            className="button primary"
            onClick={() => onAction(`/jobs/${job.id}/retry`)}
          >
            <RefreshCw size={15} />
            {t("다시 녹화")}
          </button>
        </div>
      )}
    </Modal>
  );
}

function AppPreferences() {
  const { language, setLanguage, t } = useI18n();
  return (
    <section className="settings-card app-preferences">
      <h2>{t("앱 설정")}</h2>
      <label className="field">
        {t("언어")}
        <select
          aria-label={t("언어")}
          value={language}
          onChange={(e) => setLanguage(e.target.value as Language)}
        >
          <option value="en" lang="en">
            English
          </option>
          <option value="ko" lang="ko">
            한국어
          </option>
        </select>
      </label>
      <p className="muted">{t("이 앱에 바로 적용되며 자동으로 저장됩니다.")}</p>
    </section>
  );
}

function SettingsView({
  snapshot,
  remoteMode,
  busy,
  onAction,
  onNotify,
}: {
  snapshot: Snapshot;
  remoteMode: boolean;
  busy: boolean;
  onAction: (
    path: string,
    method?: string,
    body?: unknown,
    success?: string,
  ) => Promise<void>;
  onNotify: (message: string, error?: boolean) => void;
}) {
  const { t } = useI18n();
  const [settings, setSettings] = useState<Settings>(snapshot.settings);
  const [dirty, setDirty] = useState(false);
  useEffect(() => {
    if (!dirty) setSettings(snapshot.settings);
  }, [snapshot.settings, dirty]);
  function update<K extends keyof Settings>(key: K, value: Settings[K]) {
    setSettings((s) => ({ ...s, [key]: value }));
    setDirty(true);
  }
  async function choose(
    key: "storage_root" | "backup_root" | "cookies_path" | "po_token_path",
  ) {
    try {
      const path = await open({
        directory: key !== "cookies_path" && key !== "po_token_path",
        multiple: false,
        filters:
          key === "cookies_path" || key === "po_token_path"
            ? [{ name: "text", extensions: ["txt"] }]
            : undefined,
      });
      if (path) update(key, path);
    } catch (e) {
      onNotify(String(e), true);
    }
  }
  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      await onAction(
        "/settings",
        "PUT",
        {
          ...settings,
          backup_root: settings.backup_root?.trim()
            ? settings.backup_root
            : null,
          cookies_path: settings.cookies_path?.trim()
            ? settings.cookies_path
            : null,
          replica_remote: settings.replica_remote?.trim()
            ? settings.replica_remote
            : null,
          po_token_path: settings.po_token_path?.trim()
            ? settings.po_token_path
            : null,
        },
        "설정을 저장했습니다. 저장 위치는 새 작업부터 적용됩니다.",
      );
      setDirty(false);
      if (settings.notifications && !(await isPermissionGranted()))
        await requestPermission();
    } catch {
      /* action displays errors */
    }
  }
  return (
    <div className="settings-content">
      <form onSubmit={submit}>
        <AutomationFields
          value={settings.automation}
          onChange={(value) => update("automation", value)}
        />
        <section className="settings-card">
          <div className="settings-heading">
            <HardDrive size={19} />
            <div>
              <h2>{t("녹화와 저장")}</h2>
              <p>{t("저장 위치와 동시 녹화 수")}</p>
            </div>
          </div>
          <label className="field">
            {t("저장 폴더")}
            <div className="input-group">
              <input
                required
                value={settings.storage_root}
                onChange={(e) => update("storage_root", e.target.value)}
              />
              <button
                type="button"
                className="button"
                disabled={remoteMode}
                onClick={() => void choose("storage_root")}
              >
                <FolderOpen size={16} />
                {t("찾기")}
              </button>
            </div>
          </label>
          <label className="field">
            {t("별도 백업 폴더")}
            <div className="input-group">
              <input
                placeholder={t("비워 두면 백업하지 않습니다")}
                value={settings.backup_root ?? ""}
                onChange={(e) => update("backup_root", e.target.value || null)}
              />
              <button
                type="button"
                className="button"
                disabled={remoteMode}
                onClick={() => void choose("backup_root")}
              >
                <FolderOpen size={16} />
                {t("찾기")}
              </button>
            </div>
          </label>
          <p className="muted">
            {t(
              "확정된 원본만 복사합니다. 백업 실패는 녹화를 중단하지 않습니다.",
            )}
          </p>
          <label className="field">
            {t("유튜브 쿠키 파일")}
            <div className="input-group">
              <input
                placeholder={t("Netscape cookies.txt · 회원 전용·연령 제한용")}
                value={settings.cookies_path ?? ""}
                onChange={(e) => update("cookies_path", e.target.value || null)}
              />
              <button
                type="button"
                className="button"
                disabled={remoteMode}
                onClick={() => void choose("cookies_path")}
              >
                <FolderOpen size={16} />
                {t("찾기")}
              </button>
            </div>
          </label>
          <p className="muted">
            {t(
              "브라우저에서 내보낸 Netscape 형식만 사용합니다. 쿠키 내용은 로그에 남기지 않습니다.",
            )}
          </p>
          <label className="field">
            {t("PO Token 파일")}
            <div className="input-group">
              <input
                placeholder={t("한 줄짜리 토큰 파일 · 비우면 사용 안 함")}
                value={settings.po_token_path ?? ""}
                onChange={(e) =>
                  update("po_token_path", e.target.value || null)
                }
              />
              <button
                type="button"
                className="button"
                disabled={remoteMode}
                onClick={() => void choose("po_token_path")}
              >
                <FolderOpen size={16} />
                {t("찾기")}
              </button>
            </div>
          </label>
          <p className="muted">
            {t("토큰 값은 녹화 폴더·로그에 넣지 않습니다. 파일만 지정하세요.")}
          </p>
          <label className="field">
            {t("이중 녹화 원격")}
            <select
              value={settings.replica_remote ?? ""}
              onChange={(e) => update("replica_remote", e.target.value || null)}
            >
              <option value="">{t("사용 안 함")}</option>
              {(snapshot.replica_targets ?? []).map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
          </label>
          <p className="muted">
            {t(
              "현재 선택한 녹화 장비에 등록된 원격에 요청합니다. 상대 장비의 수신 상태를 별도로 확인합니다.",
            )}
          </p>
          <div className="field-grid">
            <label className="field">
              {t("동시 녹화 수")}
              <input
                type="number"
                min="1"
                max="16"
                required
                value={settings.max_recordings}
                onChange={(e) =>
                  update("max_recordings", Number(e.target.value))
                }
              />
            </label>
            <label className="field">
              {t("최소 여유 공간 (GiB)")}
              <input
                type="number"
                min="0"
                step="any"
                required
                value={settings.min_free_bytes / 1024 ** 3}
                onChange={(e) =>
                  update(
                    "min_free_bytes",
                    Math.round(Number(e.target.value) * 1024 ** 3),
                  )
                }
              />
            </label>
            <label className="field">
              {t("채널 감시 주기 (초)")}
              <input
                type="number"
                min="15"
                max="3600"
                required
                value={settings.scan_interval_secs}
                onChange={(e) =>
                  update("scan_interval_secs", Number(e.target.value))
                }
              />
            </label>
            <label className="field">
              {t("수신 정지 감지 (초)")}
              <input
                type="number"
                min="30"
                max="1800"
                required
                value={settings.stall_timeout_secs}
                onChange={(e) =>
                  update("stall_timeout_secs", Number(e.target.value))
                }
              />
            </label>
          </div>
          <label className="check-field">
            <input
              type="checkbox"
              checked={settings.live_from_start}
              onChange={(e) => update("live_from_start", e.target.checked)}
            />
            <div>
              {t("가능하면 방송 처음부터 저장")}
              <small>
                {t("과거 구간의 접근 가능 여부에 따라 달라집니다.")}
              </small>
            </div>
          </label>
          <label className="check-field">
            <input
              type="checkbox"
              checked={settings.close_to_tray}
              onChange={(e) => update("close_to_tray", e.target.checked)}
            />
            <div>
              {t("창을 닫으면 트레이로 이동")}
              <small>
                {t(
                  "GUI 종료와 녹화 중지는 별개입니다. 녹화 화면에서 작업을 중지하세요.",
                )}
              </small>
            </div>
          </label>
          <label className="check-field">
            <input
              type="checkbox"
              checked={settings.notifications}
              onChange={(e) => update("notifications", e.target.checked)}
            />
            <div>{t("완료·부분 보관·오류 알림")}</div>
          </label>
          <div className="settings-save">
            <span>
              {t(
                dirty
                  ? "저장하지 않은 변경사항이 있습니다."
                  : "설정이 저장되어 있습니다.",
              )}
            </span>
            <button className="button primary" disabled={busy || !dirty}>
              <Check size={16} />
              {t("설정 저장")}
            </button>
          </div>
        </section>
      </form>
      <section className="settings-card">
        <div className="settings-heading">
          <Terminal size={19} />
          <div>
            <h2>{t("녹화 엔진")}</h2>
            <p>{t("진행 중인 작업에는 적용되지 않습니다.")}</p>
          </div>
        </div>
        {snapshot.tools.map((tool) => (
          <div className="tool-row" key={tool.name}>
            <div className={`tool-icon ${tool.error ? "missing" : ""}`}>
              {tool.error ? (
                <AlertCircle size={17} />
              ) : (
                <CheckCircle2 size={17} />
              )}
            </div>
            <div>
              <strong>{tool.name}</strong>
              <p>{tool.version || (tool.error && t(tool.error))}</p>
            </div>
            <span className="tool-source">
              {t(
                tool.managed
                  ? "앱 관리"
                  : tool.path
                    ? "내장 / 시스템"
                    : "미설치",
              )}
            </span>
          </div>
        ))}
        {snapshot.tool_message && (
          <p className="tool-message">
            {snapshot.installing_tools && (
              <Loader2 className="spin" size={15} />
            )}{" "}
            {t(snapshot.tool_message)}
          </p>
        )}
        <div className="settings-actions">
          <button
            className="button"
            disabled={snapshot.installing_tools || busy}
            onClick={() => void onAction("/tools/refresh").catch(() => {})}
          >
            <RefreshCw size={15} />
            {t("상태 확인")}
          </button>
          <button
            className="button"
            disabled={snapshot.installing_tools || busy}
            onClick={() =>
              void onAction(
                "/tools/rollback",
                "POST",
                {},
                "이전 엔진으로 전환했습니다.",
              ).catch(() => {})
            }
          >
            {t("이전 버전 복원")}
          </button>
          <button
            className="button primary"
            disabled={snapshot.installing_tools || busy}
            onClick={() => void onAction("/tools/install").catch(() => {})}
          >
            <Download size={15} />
            {t(snapshot.installing_tools ? "설치 중…" : "검증된 엔진 설치")}
          </button>
        </div>
      </section>
      <div className="settings-footer">
        <span>YTLR {snapshot.version}</span>
      </div>
    </div>
  );
}
