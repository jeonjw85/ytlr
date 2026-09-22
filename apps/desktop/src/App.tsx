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
  ChevronRight,
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
import {
  bytes,
  duration,
  elapsed,
  isTerminal,
  stateLabels,
  type Job,
  type JobEvent,
  type Remote,
  type Settings,
  type Snapshot,
} from "./types";

type Page = "recordings" | "channels" | "library" | "settings";
const pageInfo: Record<Page, [string, string]> = {
  recordings: ["녹화", "놓치고 싶지 않은 순간을, 안전하게."],
  channels: ["채널", "방송이 시작되면 알아서 기록합니다."],
  library: ["보관함", "저장된 방송과 검증 결과를 확인하세요."],
  settings: ["설정", "내 환경에 맞는 녹화 공간을 만드세요."],
};

export default function App() {
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
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [clock, setClock] = useState(Date.now());
  const previous = useRef<Map<string, string>>(new Map());
  const initialized = useRef(false);
  const fetching = useRef(false);
  const connectionEpoch = useRef(0);

  const notify = useCallback(
    (message: string, error = false) => setToast({ message, error }),
    [],
  );
  const refresh = useCallback(async () => {
    if (!isTauri() || fetching.current) return;
    fetching.current = true;
    const epoch = connectionEpoch.current;
    try {
      const next = await api<Snapshot>("/snapshot");
      if (epoch !== connectionEpoch.current) return;
      setSnapshot(next);
      setConnectionError("");
      if (initialized.current && next.settings.notifications) {
        for (const job of next.jobs) {
          if (
            previous.current.get(job.id) !== job.state &&
            ["completed", "partial", "failed"].includes(job.state)
          ) {
            if (await isPermissionGranted())
              sendNotification({
                title: `${stateLabels[job.state]} · YTLiveRecord`,
                body: job.title,
              });
          }
        }
      }
      previous.current = new Map(next.jobs.map((j) => [j.id, j.state]));
      initialized.current = true;
    } catch (error) {
      if (epoch === connectionEpoch.current) setConnectionError(String(error));
    } finally {
      if (epoch === connectionEpoch.current) fetching.current = false;
    }
  }, []);

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
    initialized.current = false;
    setSnapshot(null);
    setDetail(null);
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
      throw new Error("연결 상태를 확인한 뒤 다시 시도하세요.");
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

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">
            <Radio size={23} strokeWidth={2.4} />
          </div>
          <div>
            YTLiveRecord<small>KEEP THE MOMENT</small>
          </div>
        </div>
        <div className="nav-label">WORKSPACE</div>
        <nav aria-label="주 메뉴">
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
              <span>{label}</span>
              {key === "recordings" && running.length > 0 && (
                <span className="nav-count">{running.length}</span>
              )}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="local-status">
            <span
              className={`status-light ${connectionError ? "offline" : ""}`}
            />
            <div>
              {connectionError
                ? "연결 확인 필요"
                : snapshot
                  ? connection === "local"
                    ? "로컬 녹화 서비스"
                    : `원격 · ${connection}`
                  : "서비스 연결 중"}
              <small>
                {snapshot
                  ? connection === "local"
                    ? `v${snapshot.version} · 이 기기에서 실행`
                    : `v${snapshot.version} · SSH 터널`
                  : "녹화 상태를 가져옵니다"}
              </small>
              <select
                aria-label="연결 대상"
                value={connection}
                disabled={busy}
                onChange={(e) => void switchConnection(e.target.value)}
              >
                <option value="local">이 기기</option>
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
                원격 추가
              </button>
            </div>
          </div>
          <button
            className={`nav-item ${page === "settings" ? "selected" : ""}`}
            onClick={() => setPage("settings")}
          >
            <Settings2 size={19} />
            <span>설정</span>
          </button>
          <div className="sidebar-foot">OPEN SOURCE · BUILT TO KEEP</div>
        </div>
      </aside>

      <main>
        <header className="topbar">
          <div className="breadcrumb">
            내 워크스페이스 <ChevronRight size={14} />{" "}
            <span>{pageInfo[page][0]}</span>
          </div>
          <div className="topbar-right">
            <ShieldCheck size={15} />
            <span>
              {connection === "local"
                ? "로컬 저장"
                : `원격 저장 · ${connection}`}{" "}
              · 원본 보존
            </span>
            <span className="avatar">Y</span>
          </div>
        </header>
        <div className="content">
          <div className="page-heading">
            <div>
              <div className="eyebrow">YOUR LIVE ARCHIVE</div>
              <h1>{pageInfo[page][0]}</h1>
              <p>{pageInfo[page][1]}</p>
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
                {page === "channels" ? "채널 추가" : "새 녹화"}
              </button>
            )}
          </div>

          {!isTauri() && (
            <div className="notice">
              <Terminal size={20} />
              <div>
                <strong>데스크톱 앱에서 실행해 주세요</strong>
                <p>
                  프로젝트 루트에서 <code>pnpm dev</code>를 실행하면 GUI와 실제
                  녹화 서비스가 연결됩니다.
                </p>
              </div>
            </div>
          )}
          {connectionError && (
            <div className="notice danger">
              <WifiOff size={20} />
              <div>
                <strong>서비스에 연결할 수 없습니다</strong>
                <p>
                  녹화 상태를 확인할 수 없습니다. 마지막으로 확인한 정보를
                  표시합니다.
                </p>
                <small>{connectionError}</small>
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
                다시 연결
              </button>
            </div>
          )}
          {missingTools && page !== "settings" && (
            <div className="notice">
              <Download size={20} />
              <div>
                <strong>첫 녹화를 위한 준비가 필요해요</strong>
                <p>
                  설정에서 녹화 엔진을 한 번 설치하면 Python 설정 없이 사용할 수
                  있습니다.
                </p>
              </div>
              <button
                className="button small"
                onClick={() => setPage("settings")}
              >
                엔진 설정
                <ArrowUpRight size={14} />
              </button>
            </div>
          )}

          {page === "recordings" && (
            <>
              <div className="stats">
                <Stat
                  icon={<Radio size={19} />}
                  label="지금 녹화 중"
                  value={`${recordingCount}`}
                  suffix={`/ ${snapshot?.settings.max_recordings ?? 2}`}
                  detail={
                    running.length > recordingCount
                      ? `${running.length - recordingCount}개 작업 대기·처리 중`
                      : "새로운 방송을 기다리고 있어요"
                  }
                  accent
                />
                <Stat
                  icon={<HardDrive size={19} />}
                  label="녹화 자료"
                  value={bytes(jobs.reduce((n, j) => n + j.bytes, 0))}
                  detail="원본·조각·결과 파일 포함"
                />
                <Stat
                  icon={<ShieldCheck size={19} />}
                  label="남은 저장 공간"
                  value={bytes(snapshot?.free_bytes ?? null)}
                  detail="녹화 전에 여유 공간을 확인합니다"
                />
              </div>
              <div className="section-bar">
                <h2>
                  진행 중인 작업 <span>{running.length}</span>
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
                      busy={busy}
                    />
                  ))}
                </div>
              ) : (
                <Empty
                  icon={<Radio size={32} />}
                  title={
                    search
                      ? "검색 결과가 없습니다"
                      : "다음 라이브를 기록해 보세요"
                  }
                  text="라이브 URL을 붙여넣거나 채널을 등록하면, 방송을 기록하고 원본을 보관합니다."
                  action={
                    <button
                      className="button primary"
                      disabled={!snapshot}
                      onClick={() => setAdd("record")}
                    >
                      <Plus size={16} />첫 녹화 추가
                    </button>
                  }
                />
              )}
              <div className="tip">
                <ShieldCheck size={17} />
                <span>
                  창을 닫아도 녹화는 계속됩니다. 진행 중인 작업은 이 기기의
                  백그라운드 서비스가 관리합니다.
                </span>
              </div>
            </>
          )}

          {page === "channels" && (
            <>
              <div className="section-bar">
                <h2>
                  등록한 채널 <span>{snapshot?.channels.length ?? 0}</span>
                </h2>
                <span className="muted">
                  {snapshot?.settings.scan_interval_secs ?? 60}초 간격으로 확인
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
                          aria-label={`${channel.name} 자동 녹화`}
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
                        {channel.enabled ? "방송 자동 감시" : "감시 일시 중지"}
                      </div>
                      {channel.last_error && (
                        <p className="inline-error">{channel.last_error}</p>
                      )}
                      <div className="channel-footer">
                        <span>
                          {channel.last_checked_at
                            ? `마지막 확인 ${new Date(channel.last_checked_at).toLocaleTimeString("ko-KR")}`
                            : "첫 확인 대기"}
                        </span>
                        <button
                          className="icon-button"
                          aria-label={`${channel.name} 삭제`}
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
                  icon={<Radio size={32} />}
                  title="채널을 등록하면 더 편해져요"
                  text="채널의 라이브 목록을 주기적으로 확인하고, 새로운 방송을 자동으로 녹화합니다."
                  action={
                    <button
                      className="button primary"
                      disabled={!snapshot}
                      onClick={() => setAdd("channel")}
                    >
                      <Plus size={16} />
                      채널 추가
                    </button>
                  }
                />
              )}
            </>
          )}

          {page === "library" && (
            <>
              <div className="section-bar">
                <h2>
                  보관된 작업 <span>{jobs.filter(isTerminal).length}</span>
                </h2>
                <SearchBox value={search} onChange={setSearch} />
              </div>
              {shown.length ? (
                <div className="library-list">
                  {shown.map((job) => (
                    <div className="library-row" key={job.id}>
                      <div className="file-icon">
                        <FileVideo2 size={25} />
                      </div>
                      <div className="library-title">
                        <h3>{job.title}</h3>
                        <p>
                          {job.channel || job.video_id} ·{" "}
                          {new Date(job.created_at).toLocaleDateString("ko-KR")}{" "}
                          · {bytes(job.bytes)}
                        </p>
                        <span className="library-note">{job.message}</span>
                      </div>
                      <Badge job={job} />
                      <button
                        className="icon-button"
                        title="폴더 열기"
                        aria-label="폴더 열기"
                        onClick={() => safeOpen(job)}
                      >
                        <FolderOpen size={19} />
                      </button>
                      <button
                        className="button small"
                        onClick={() => setDetail(job.id)}
                      >
                        상세
                        <ChevronRight size={14} />
                      </button>
                    </div>
                  ))}
                </div>
              ) : (
                <Empty
                  icon={<FolderOpen size={32} />}
                  title="기록이 쌓이는 공간"
                  text="녹화가 끝나면 파일과 검증 결과를 이곳에서 확인할 수 있습니다."
                />
              )}
            </>
          )}

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
              <p>녹화 서비스를 준비하고 있습니다…</p>
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
          fromStart={snapshot?.settings.live_from_start ?? true}
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
        />
      )}
      {toast && (
        <div
          className={`toast ${toast.error ? "error" : ""}`}
          role={toast.error ? "alert" : "status"}
        >
          {toast.error ? <AlertCircle size={18} /> : <CheckCircle2 size={18} />}
          <span>{toast.message}</span>
          <button
            className="icon-button"
            aria-label="알림 닫기"
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
  icon,
  label,
  value,
  suffix,
  detail,
  accent = false,
}: {
  icon: ReactNode;
  label: string;
  value: string;
  suffix?: string;
  detail: string;
  accent?: boolean;
}) {
  return (
    <div className={`stat ${accent ? "accent" : ""}`}>
      <div className="stat-top">
        <span>{label}</span>
        {icon}
      </div>
      <div className="stat-value">
        {value}
        <small>{suffix}</small>
      </div>
      <div className="stat-detail">
        {accent && <span className="mini-dot" />}
        {detail}
      </div>
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
  return (
    <label className="search">
      <Search size={15} />
      <input
        aria-label="녹화 검색"
        placeholder="제목 또는 채널 검색"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}
function Empty({
  icon,
  title,
  text,
  action,
}: {
  icon: ReactNode;
  title: string;
  text: string;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <div className="empty-orbit">
        <span />
        {icon}
      </div>
      <h2>{title}</h2>
      <p>{text}</p>
      {action}
    </div>
  );
}
function Badge({ job }: { job: Job }) {
  return (
    <span className={`badge ${job.state}`}>
      {job.state === "recording" ? (
        <span className="live-dot" />
      ) : job.state === "completed" ? (
        <Check size={12} />
      ) : job.state === "reconnecting" ? (
        <RefreshCw size={12} />
      ) : (
        <Circle size={9} />
      )}{" "}
      {stateLabels[job.state]}
    </span>
  );
}
function JobCard({
  job,
  clock,
  onDetail,
  onFolder,
  onStop,
  busy,
}: {
  job: Job;
  clock: number;
  onDetail: () => void;
  onFolder: () => void;
  onStop: () => void;
  busy: boolean;
}) {
  return (
    <article
      className={`job-card ${job.state === "recording" ? "is-recording" : ""}`}
    >
      <div className="job-top">
        <div className="job-art">
          <Radio size={29} />
          {job.state === "recording" && (
            <div className="wave">
              {Array.from({ length: 12 }, (_, i) => (
                <i
                  key={i}
                  style={{
                    height: `${8 + ((i * 17) % 30)}px`,
                    animationDelay: `${i * 0.1}s`,
                  }}
                />
              ))}
            </div>
          )}
        </div>
        <div className="job-title">
          <div className="job-channel">
            {job.channel || "YOUTUBE LIVE"}
            <Badge job={job} />
          </div>
          <h3>{job.title}</h3>
          <p>{job.format || job.url}</p>
        </div>
        <button
          className="icon-button"
          title="폴더 열기"
          aria-label="폴더 열기"
          onClick={onFolder}
        >
          <FolderOpen size={19} />
        </button>
      </div>
      <div className="job-metrics">
        <div>
          <Clock3 size={15} />
          <span>경과 시간</span>
          <strong>{elapsed(job, clock)}</strong>
        </div>
        <div>
          <HardDrive size={15} />
          <span>저장 자료</span>
          <strong>{bytes(job.bytes)}</strong>
        </div>
        <div>
          <Activity size={15} />
          <span>최근 수신</span>
          <strong>
            {job.last_media_at
              ? `${Math.max(0, Math.floor((clock - new Date(job.last_media_at).getTime()) / 1000))}초 전`
              : "대기"}
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
          {job.message}
        </span>
        <div>
          <button className="button ghost small" onClick={onDetail}>
            상세
          </button>
          <button
            className="button small stop"
            disabled={busy || job.stop_requested}
            onClick={onStop}
          >
            <Square size={11} fill="currentColor" />
            {job.stop_requested ? "중지 중" : "중지"}
          </button>
        </div>
      </div>
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
          <button className="icon-button" aria-label="닫기" onClick={onClose}>
            <X size={20} />
          </button>
        </div>
        {children}
      </div>
    </div>
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
  const [url, setUrl] = useState("");
  const [name, setName] = useState("");
  const [start, setStart] = useState(fromStart);
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
          ? { url, live_from_start: start, priority: 0 }
          : { url, name, priority: 0 },
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setPending(false);
    }
  }
  return (
    <Modal
      title={kind === "record" ? "새 라이브 녹화" : "채널 추가"}
      onClose={onClose}
    >
      <form onSubmit={submit}>
        <p className="modal-intro">
          {kind === "record"
            ? "진행 중인 라이브 또는 예약 방송의 URL을 입력하세요."
            : "등록한 채널의 새 라이브를 자동으로 녹화합니다."}
        </p>
        <label className="field">
          {kind === "record" ? "영상 URL" : "채널 URL"}
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
        {kind === "channel" ? (
          <label className="field">
            표시 이름 <span className="muted">선택</span>
            <input
              placeholder="알아보기 쉬운 채널 이름"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </label>
        ) : (
          <label className="check-field">
            <input
              type="checkbox"
              checked={start}
              onChange={(e) => setStart(e.target.checked)}
            />
            <div>
              가능하면 방송 처음부터 저장
              <small>
                유튜브에서 제공하는 과거 구간 범위에 따라 달라집니다.
              </small>
            </div>
          </label>
        )}
        <div className="modal-hint">
          <ShieldCheck size={17} />
          <span>최고 화질 · 원본 보존 · 자동 재연결</span>
        </div>
        {error && (
          <p className="inline-error" role="alert">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="button" onClick={onClose}>
            취소
          </button>
          <button className="button primary" disabled={pending}>
            {pending ? (
              <Loader2 className="spin" size={16} />
            ) : (
              <Plus size={16} />
            )}
            추가하기
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
    <Modal title="SSH 원격 추가" onClose={onClose}>
      <form onSubmit={submit}>
        <p className="modal-intro">
          서버에서 녹화 서비스가 localhost로만 열립니다. 이 앱은 SSH 터널로
          연결합니다. 비밀번호는 저장하지 않으므로 키 로그인이 필요합니다.
        </p>
        <label className="field">
          이름
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
          서버 데이터 폴더
          <input
            required
            value={dataDir}
            onChange={(e) => setDataDir(e.target.value)}
            placeholder="/home/ytlr/.local/share/YTLiveRecord/YTLiveRecord"
          />
        </label>
        <label className="field">
          SSH 키 <span className="muted">선택</span>
          <input
            value={identity}
            onChange={(e) => setIdentity(e.target.value)}
            placeholder="비우면 ssh 기본 키"
          />
        </label>
        <label className="field">
          서버 ytlr 실행 파일 · 선택
          <input
            value={serverExecutable}
            onChange={(e) => setServerExecutable(e.target.value)}
            placeholder="/home/user/.local/bin/ytlr · 비우면 ytlr"
          />
        </label>
        <label className="field">
          known_hosts 파일 · 선택
          <input
            value={knownHosts}
            onChange={(e) => setKnownHosts(e.target.value)}
            placeholder="비우면 SSH 기본 호스트 키 저장소"
          />
        </label>
        {error && (
          <p className="inline-error" role="alert">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="button" onClick={onClose}>
            취소
          </button>
          <button className="button primary" disabled={pending}>
            {pending ? <Loader2 className="spin" size={16} /> : "저장"}
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
}) {
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
    <Modal title="녹화 상세" onClose={onClose}>
      <div className="detail-title">
        <Badge job={job} />
        <h3>{job.title}</h3>
        <p>{job.message}</p>
      </div>
      <dl className="detail-grid">
        <div>
          <dt>수집 시도</dt>
          <dd>{job.attempt}회</dd>
        </div>
        <div>
          <dt>재시도</dt>
          <dd>{job.retries}회</dd>
        </div>
        <div>
          <dt>자료 용량</dt>
          <dd>{bytes(job.bytes)}</dd>
        </div>
        <div>
          <dt>연속성</dt>
          <dd>{job.continuity_uncertain ? "확인 필요" : "확인된 누락 없음"}</dd>
        </div>
      </dl>
      {(job.gaps ?? []).length > 0 && (
        <>
          <h4>추정 수신 공백 · 접합 경계 미검증</h4>
          {(job.gaps ?? []).map((gap) => (
            <p
              className="library-note"
              key={`${gap.after_attempt}-${gap.started_at}`}
            >
              시도 {gap.after_attempt} 이후 {duration(gap.seconds)} ·{" "}
              {gap.status === "recovered"
                ? "복구됨"
                : gap.status === "collected"
                  ? "보충 파일 확보 · 경계 미검증"
                  : gap.status === "unrecoverable"
                    ? "복구 불가"
                    : "미복구"}
            </p>
          ))}
        </>
      )}
      <div className="path-box">
        {job.output_dir}
        <button
          className="icon-button"
          title="폴더 열기"
          onClick={() => onOpen(job)}
        >
          <FolderOpen size={17} />
        </button>
      </div>
      {job.backup?.checked_at && (
        <p className={job.backup.state === "failed" ? "inline-error" : "muted"}>
          백업{" "}
          {job.backup.state === "verified"
            ? "검증 완료"
            : job.backup.state === "failed"
              ? "재시도 필요"
              : "진행 중"}{" "}
          · {job.backup.verified_files}개 파일 · {job.backup.message}
        </p>
      )}
      {job.replica && (
        <p className="muted">
          이중 녹화 · {job.replica.target} · {job.replica.state} ·{" "}
          {job.replica.message}
        </p>
      )}
      <h4>재생 가능한 결과</h4>
      {job.outputs.length ? (
        job.outputs.map((output, i) => (
          <div className="output-row" key={output.path}>
            <FileVideo2 size={18} />
            <div>
              <strong>{output.path.split(/[\\/]/).pop()}</strong>
              <small>{bytes(output.bytes)} · 영상·음성 헤더 검사</small>
            </div>
            <button className="button small" onClick={() => onOpen(job, i)}>
              열기
            </button>
            <button
              className="icon-button"
              title="MP4 내보내기"
              aria-label="MP4 내보내기"
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
          아직 검증된 결과 파일이 없습니다. 수집 원본은 녹화 폴더에 보존됩니다.
        </p>
      )}
      <div className="section-bar compact">
        <h4>이벤트</h4>
        <button className="button ghost small" onClick={() => void exportLog()}>
          <Download size={14} />
          로그 저장
        </button>
      </div>
      <div className="events">
        {events.length ? (
          events.map((e) => (
            <div className="event" key={e.id}>
              <time>{new Date(e.at).toLocaleTimeString("ko-KR")}</time>
              <span>{e.message}</span>
            </div>
          ))
        ) : (
          <p className="muted">기록된 이벤트가 없습니다.</p>
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
            원본 복구
          </button>
          <button
            className="button primary"
            onClick={() => onAction(`/jobs/${job.id}/retry`)}
          >
            <RefreshCw size={15} />
            다시 녹화
          </button>
        </div>
      )}
    </Modal>
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
        <section className="settings-card">
          <div className="settings-heading">
            <HardDrive size={19} />
            <div>
              <h2>녹화와 저장</h2>
              <p>최고 화질로 수집하고, 원본 자료를 유지합니다.</p>
            </div>
          </div>
          <label className="field">
            저장 폴더
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
                찾기
              </button>
            </div>
          </label>
          <label className="field">
            별도 백업 폴더
            <div className="input-group">
              <input
                placeholder="비워 두면 백업하지 않습니다"
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
                찾기
              </button>
            </div>
          </label>
          <p className="muted">
            확정된 원본만 복사합니다. 백업 실패는 녹화를 중단하지 않습니다.
          </p>
          <label className="field">
            유튜브 쿠키 파일
            <div className="input-group">
              <input
                placeholder="Netscape cookies.txt · 회원 전용·연령 제한용"
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
                찾기
              </button>
            </div>
          </label>
          <p className="muted">
            브라우저에서 내보낸 Netscape 형식만 사용합니다. 쿠키 내용은 로그에
            남기지 않습니다.
          </p>
          <label className="field">
            PO Token 파일
            <div className="input-group">
              <input
                placeholder="한 줄짜리 토큰 파일 · 비우면 사용 안 함"
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
                찾기
              </button>
            </div>
          </label>
          <p className="muted">
            토큰 값은 녹화 폴더·로그에 넣지 않습니다. 파일만 지정하세요.
          </p>
          <label className="field">
            이중 녹화 원격
            <select
              value={settings.replica_remote ?? ""}
              onChange={(e) => update("replica_remote", e.target.value || null)}
            >
              <option value="">사용 안 함</option>
              {(snapshot.replica_targets ?? []).map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
          </label>
          <p className="muted">
            현재 선택한 녹화 장비에 등록된 원격에 요청합니다. 상대 장비의 수신
            상태를 별도로 확인합니다.
          </p>
          <div className="field-grid">
            <label className="field">
              동시 녹화 수
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
              최소 여유 공간 (GiB)
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
              채널 감시 주기 (초)
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
              수신 정지 감지 (초)
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
              가능하면 방송 처음부터 저장
              <small>과거 구간의 접근 가능 여부에 따라 달라집니다.</small>
            </div>
          </label>
          <label className="check-field">
            <input
              type="checkbox"
              checked={settings.close_to_tray}
              onChange={(e) => update("close_to_tray", e.target.checked)}
            />
            <div>
              창을 닫으면 트레이로 이동
              <small>
                GUI 종료와 녹화 중지는 별개입니다. 녹화 화면에서 작업을
                중지하세요.
              </small>
            </div>
          </label>
          <label className="check-field">
            <input
              type="checkbox"
              checked={settings.notifications}
              onChange={(e) => update("notifications", e.target.checked)}
            />
            <div>완료·부분 보관·오류 알림</div>
          </label>
          <div className="settings-save">
            <span>
              {dirty
                ? "저장하지 않은 변경사항이 있습니다."
                : "설정이 저장되어 있습니다."}
            </span>
            <button className="button primary" disabled={busy || !dirty}>
              <Check size={16} />
              설정 저장
            </button>
          </div>
        </section>
      </form>
      <section className="settings-card">
        <div className="settings-heading">
          <Terminal size={19} />
          <div>
            <h2>녹화 엔진</h2>
            <p>
              검증된 버전과 체크섬을 사용합니다. 진행 중인 작업에는 변경을
              적용하지 않습니다.
            </p>
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
              <p>{tool.version || tool.error}</p>
            </div>
            <span className="tool-source">
              {tool.managed
                ? "앱 관리"
                : tool.path
                  ? "내장 / 시스템"
                  : "미설치"}
            </span>
          </div>
        ))}
        {snapshot.tool_message && (
          <p className="tool-message">
            {snapshot.installing_tools && (
              <Loader2 className="spin" size={15} />
            )}{" "}
            {snapshot.tool_message}
          </p>
        )}
        <div className="settings-actions">
          <button
            className="button"
            disabled={snapshot.installing_tools || busy}
            onClick={() => void onAction("/tools/refresh").catch(() => {})}
          >
            <RefreshCw size={15} />
            상태 확인
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
            이전 버전 복원
          </button>
          <button
            className="button primary"
            disabled={snapshot.installing_tools || busy}
            onClick={() => void onAction("/tools/install").catch(() => {})}
          >
            <Download size={15} />
            {snapshot.installing_tools ? "설치 중…" : "검증된 엔진 설치"}
          </button>
        </div>
      </section>
      <div className="settings-footer">
        <Radio size={17} />
        <span>YTLiveRecord {snapshot.version}</span>
        <span>Rust + Tauri · 로컬 우선 라이브 아카이브</span>
      </div>
    </div>
  );
}
