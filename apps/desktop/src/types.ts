export type JobState =
  | "queued"
  | "waiting"
  | "preparing"
  | "recording"
  | "reconnecting"
  | "finalizing"
  | "completed"
  | "partial"
  | "stopped"
  | "failed";
export interface MediaOutput {
  path: string;
  bytes: number;
  duration: number;
  has_video: boolean;
  has_audio: boolean;
  verification: string;
}
export interface RecordingOptions {
  audio_only: boolean;
  max_height: 480 | 720 | 1080 | null;
}
export const defaultRecordingOptions: RecordingOptions = {
  audio_only: false,
  max_height: null,
};
export const channelErrorLabels: Record<string, string> = {
  authentication: "인증 오류",
  network: "네트워크 오류",
  engine: "녹화 엔진 오류",
  extraction: "방송 정보 조회 오류",
};
export interface Job {
  schedule?: RecordingSchedule;
  protected?: boolean;
  finished_at?: string | null;
  stop_at?: string | null;
  bookmarks?: Bookmark[];
  alerts?: JobAlert[];
  recovery_error?: string | null;
  recording_options?: RecordingOptions;
  resolution?: string | null;
  id: string;
  url: string;
  video_id: string;
  title: string;
  channel: string;
  state: JobState;
  message: string;
  created_at: string;
  updated_at: string;
  started_at: string | null;
  last_media_at: string | null;
  output_dir: string;
  format: string;
  bytes: number;
  media_seconds: number;
  attempt: number;
  retries: number;
  priority: number;
  live_from_start: boolean;
  continuity_uncertain: boolean;
  stop_requested: boolean;
  outputs: MediaOutput[];
  attempts?: CaptureAttempt[];
  gaps?: TimelineGap[];
  replica?: {
    target: string;
    state: string;
    remote_state: JobState | null;
    checked_at: string | null;
    message: string;
  } | null;
  backup?: {
    state: string;
    checked_at: string | null;
    message: string;
    destination: string | null;
    verified_files: number;
  };
}
export interface CaptureAttempt {
  n: number;
  started_at: string;
  ended_at: string | null;
  media_seconds: number;
  bytes: number;
}
export type GapStatus =
  "unrecovered" | "recovered" | "unrecoverable" | "collected";
export interface TimelineGap {
  after_attempt: number;
  started_at: string;
  ended_at: string | null;
  seconds: number;
  status: GapStatus;
  evidence?: {
    message: string;
    requested_start: string;
    requested_end: string;
    output: MediaOutput;
  } | null;
}
export interface Channel {
  health?: {
    consecutive_failures: number;
    last_success_at: string | null;
    error_kind: string | null;
    incident_open: boolean;
  };
  rules?: ChannelRules;
  decisions?: RuleDecision[];
  recording_options?: RecordingOptions;
  live_from_start?: boolean | null;
  id: string;
  url: string;
  name: string;
  enabled: boolean;
  priority: number;
  last_checked_at: string | null;
  last_error: string | null;
}
export interface Settings {
  prevent_sleep?: boolean;
  keep_awake_waiting?: boolean;
  automation?: AutomationSettings;
  storage_root: string;
  max_recordings: number;
  scan_interval_secs: number;
  stall_timeout_secs: number;
  min_free_bytes: number;
  max_retries: number;
  live_from_start: boolean;
  notifications: boolean;
  close_to_tray: boolean;
  backup_root?: string | null;
  cookies_path?: string | null;
  replica_remote?: string | null;
  po_token_path?: string | null;
}
export interface Tool {
  name: string;
  path: string | null;
  version: string | null;
  managed: boolean;
  error: string | null;
}
export interface Snapshot {
  operations?: Operation[];
  power?: {
    requested: boolean;
    active: boolean;
    waiting_jobs: number;
    watching_channels?: number;
    message: string;
  };
  storage?: StorageStatus[];
  version: string;
  jobs: Job[];
  channels: Channel[];
  settings: Settings;
  free_bytes: number | null;
  tools: Tool[];
  installing_tools: boolean;
  tool_message: string | null;
  replica_targets?: string[];
}
export interface StorageStatus {
  path: string;
  free_bytes: number | null;
  total_bytes: number | null;
  bytes_per_second: number | null;
  remaining_seconds: number | null;
  low_space: boolean;
}
export interface Remote {
  id: string;
  name: string;
  ssh: string;
  identity?: string | null;
  remote_data_dir: string;
  known_hosts?: string | null;
  executable?: string | null;
}
export interface JobEvent {
  id: number;
  job_id: string;
  at: string;
  kind: string;
  message: string;
}

export interface Bookmark {
  id: string;
  title: string;
  note: string;
  created_at: string;
  attempt: number;
  media_seconds: number | null;
  received_at: string;
}
export interface JobAlert {
  id: string;
  kind: string;
  message: string;
  active: boolean;
  opened_at: string;
  resolved_at: string | null;
}
export interface CleanupPlan {
  id: string;
  job_id: string;
  created_at: string;
  files: { path: string; bytes: number; sha256: string }[];
  retained: MediaOutput[];
  reclaimable_bytes: number;
}

export interface RecordingSchedule {
  start_at: string | null;
  duration_minutes: number | null;
}

export type OperationTask =
  | { kind: "export"; index: number }
  | {
      kind: "clip";
      request: {
        bookmark_id: string;
        end_bookmark_id: string | null;
        before_seconds: number;
        after_seconds: number;
      };
    }
  | { kind: "range_clip"; path: string; start: number; end: number }
  | { kind: "preview"; path: string }
  | { kind: "recover" }
  | { kind: "cleanup"; files: string[] }
  | { kind: "download"; path: string; remote: string };
export interface OperationRequest {
  job_id: string;
  task: OperationTask;
  source_path?: string | null;
}
export interface Operation {
  id: string;
  request: OperationRequest;
  state: "queued" | "running" | "completed" | "failed" | "cancelled";
  created_at: string;
  updated_at: string;
  attempts: number;
  cancel_requested: boolean;
  progress: number | null;
  bytes_done: number;
  total_bytes: number | null;
  message: string;
  error: string | null;
  result: {
    path?: string;
    source?: string;
    sha256?: string;
    warnings?: string[];
    pending_files?: string[];
    reclaimed_bytes?: number;
    completed?: boolean;
  } | null;
}
export interface LibraryFile {
  path: string;
  bytes: number;
  preview: boolean;
}
export interface WeeklyWindow {
  weekdays: number[];
  start_minute: number;
  end_minute: number;
  utc_offset_minutes: number;
}
export interface ChannelRules {
  include: string[];
  exclude: string[];
  window: WeeklyWindow | null;
  duration_minutes: number | null;
}
export interface RuleDecision {
  at: string;
  title: string;
  allowed: boolean;
  reason: string;
  window_start: string | null;
  stop_at: string | null;
}
export const defaultRules: ChannelRules = {
  include: [],
  exclude: [],
  window: null,
  duration_minutes: null,
};
export type NotificationTarget = { id: string } & (
  | { kind: "webhook" | "discord"; url_env: string }
  | { kind: "telegram"; token_env: string; chat_id: string }
);
export interface AutomationSettings {
  notifications: NotificationTarget[];
  retention: {
    cleanup_after_days: number | null;
    delete_after_days: number | null;
  };
}
export const defaultAutomation: AutomationSettings = {
  notifications: [],
  retention: { cleanup_after_days: null, delete_after_days: null },
};

export const stateLabels: Record<JobState, string> = {
  queued: "대기 중",
  waiting: "방송 대기",
  preparing: "연결 준비",
  recording: "녹화 중",
  reconnecting: "재연결 중",
  finalizing: "검증·마무리",
  completed: "완료",
  partial: "부분 보관",
  stopped: "저장됨",
  failed: "확인 필요",
};
export const isTerminal = (job: Job) =>
  ["completed", "partial", "stopped", "failed"].includes(job.state);
export function bytes(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return "확인 중";
  if (value < 1024) return `${value} B`;
  const n = Math.min(Math.floor(Math.log(value) / Math.log(1024)), 4);
  return `${(value / 1024 ** n).toFixed(1)} ${["B", "KiB", "MiB", "GiB", "TiB"][n]}`;
}
export function duration(seconds: number): string {
  const n = Math.max(0, Math.floor(seconds));
  return [Math.floor(n / 3600), Math.floor(n / 60) % 60, n % 60]
    .map((v) => String(v).padStart(2, "0"))
    .join(":");
}
export function elapsed(job: Job, at = Date.now()): string {
  if (isTerminal(job)) return duration(job.media_seconds);
  return job.started_at
    ? duration((at - new Date(job.started_at).getTime()) / 1000)
    : "—";
}
