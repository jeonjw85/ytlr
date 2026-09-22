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
export interface Job {
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
  id: string;
  url: string;
  name: string;
  enabled: boolean;
  priority: number;
  last_checked_at: string | null;
  last_error: string | null;
}
export interface Settings {
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

export const stateLabels: Record<JobState, string> = {
  queued: "대기 중",
  waiting: "방송 대기",
  preparing: "연결 준비",
  recording: "녹화 중",
  reconnecting: "재연결 중",
  finalizing: "검증·마무리",
  completed: "완료",
  partial: "부분 보관",
  stopped: "중지됨",
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
