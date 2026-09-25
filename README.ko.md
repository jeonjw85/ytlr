<div align="center">

# YTLR

Youtube Live Record

[![Release](https://img.shields.io/github/v/release/jeonjw85/ytlr)](https://github.com/jeonjw85/ytlr/releases/latest)
[![CI](https://github.com/jeonjw85/ytlr/actions/workflows/ci.yml/badge.svg)](https://github.com/jeonjw85/ytlr/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/github/license/jeonjw85/ytlr)](LICENSE)

</div>

[English](README.md) | 한국어

YTLR은 macOS, Linux, Windows에서 사용할 수 있는 라이브 방송 녹화 앱입니다.
방송 연결이 끊기거나 서비스가 재시작되어도 이미 받은 원본 파일을 보존합니다.
Tauri 기반 데스크톱 앱과 CLI를 제공합니다.

현재 버전은 **0.2.4**이며, 안정화 테스트를 진행하고 있습니다.

<p align="center">
  <img src="docs/recording.png" alt="YTLR 라이브 방송 녹화 화면" width="780">
</p>
<p align="center">
  <img src="docs/home.png" alt="녹화 목록" width="250">
  <img src="docs/channels.png" alt="채널 목록" width="250">
  <img src="docs/library.png" alt="보관함" width="250">
</p>

## 주요 기능

- 라이브 URL 녹화, 예약 방송 대기, 채널 모니터링
- 채널별 키워드·주간 시간대 규칙, 시작 예약과 녹화 시간 제한
- Webhook·Discord·Telegram 서비스 알림과 실패 시 재전송
- 보관함 필터·일괄 처리, 북마크 클립, 중요 녹화 보호와 보관 정책
- 서명 검증을 사용하는 앱 업데이트 확인·설치
- `yt-dlp`로 사용 가능한 최고 화질을 선택하고 FFmpeg로 재인코딩 없이 저장
- 최대 해상도 제한(1080p·720p·480p), 음성만 녹음, 채널별 기본 옵션 설정
- 재연결·재시작 후에도 작업별 녹화 옵션 유지
- 디스크 용량·최근 기록 속도·최소 여유 공간에 도달하기까지의 예상 시간 표시 및 공간 부족 경고
- 재연결 시에도 원본 조각과 30초 단위 MKV 분할 파일 보존
- 수신 중단 감지, 이벤트 기록, 진단 로그 제공
- 원본 트랙이나 저장이 끝난 분할 파일에서 재생 가능한 파일 복구
- 원본을 유지하면서 호환되는 결과를 MP4로 내보내기
- 녹화 완료 전 컨테이너와 영상·음성 헤더 검사
- 저장이 완료된 원본을 별도 디스크나 폴더에 백업하고 SHA-256으로 검증
- 인증이 필요한 방송에 Netscape 형식의 `cookies.txt`와 컨텍스트가 지정된 PO Token 파일 사용
- 인증된 SSH 터널을 통한 원격 녹화 서비스 제어
- 데스크톱 앱의 보관함에서 완료·중지된 작업과 해당 로컬 녹화 폴더 삭제

## 구성

- **녹화 서비스:** 작업, 저장소, 복구, 백그라운드 처리를 담당하는 Rust 서비스입니다.
- **데스크톱 앱:** Tauri 2 + React로 만든 UI에서 녹화, 채널, 보관함, 설정을 관리합니다.
- **CLI:** GUI 없이 같은 서비스를 제어할 수 있어 Linux 서버에서도 사용할 수 있습니다.
- **미디어 도구:** `yt-dlp`가 스트림을 선택하고 FFmpeg가 녹화와 미디어 검사를 담당합니다.
  배포 패키지에는 필요한 도구가 포함됩니다.

## 앱 자동 업데이트

서명된 배포 앱은 시작 약 30초 후와 실행 중 6시간마다 GitHub Releases의 안정 버전을
확인합니다. **설정 → 앱 업데이트**에서 자동 확인을 끄거나 수동으로 확인할 수 있습니다.
새 버전의 **다운로드 및 설치**를 누르면 진행률을 표시하고 업데이트 서명을 검증한 뒤
설치·재시작합니다. 확인 설정은 이 컴퓨터에 저장되며 원격 서버 선택과 무관합니다.

다운로드 중에는 녹화를 계속할 수 있습니다. 설치 직전에 로컬 서비스의 녹화·파일 처리·엔진
설치 여부를 확인합니다. 진행 중인 작업이 있으면 **설치 대기**로 남기며, 작업이 끝난 뒤
다시 설치 버튼을 누르면 현재 앱 실행 중 보관한 다운로드를 재사용합니다. 설치 시 로컬
서비스를 종료하고 서비스 잠금을 유지해 구버전 서비스가 다시 실행되는 것을 막습니다.
대기·예약 작업은 DB에 유지되어 업데이트한 앱 실행 후 다시 처리됩니다. 원격 서버는
별도로 업데이트해야 합니다.

- macOS: 앱 번들 업데이트, Windows: 설치 방식에 맞는 NSIS/MSI 업데이트
- Linux: AppImage 자동 업데이트 지원. `.deb`는 릴리스 페이지에서 수동 업데이트
- 개발 빌드와 업데이트 공개키가 없는 일반 로컬 빌드는 자동 업데이트 비활성화
- 자동 업데이트가 없는 구버전에서는 이 기능을 포함한 첫 배포판을 수동 설치

### 배포자 설정

업데이트 서명키를 한 번 생성합니다. 이 키는 macOS 코드 서명·공증 인증서와 별개입니다.

```sh
pnpm --dir apps/desktop tauri signer generate -w "$HOME/.tauri/ytlr.key"
```

GitHub 저장소의 **Settings → Secrets and variables → Actions**에 다음을 등록합니다.

| 종류 | 이름 | 값 |
|---|---|---|
| Variable | `YTLR_UPDATER_PUBLIC_KEY` | 생성된 `.pub` 파일의 전체 내용 |
| Secret | `TAURI_SIGNING_PRIVATE_KEY` | 개인키 파일의 전체 내용 |
| Secret | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 키 비밀번호. 비밀번호가 없으면 빈 값 |

개인키는 저장소에 넣지 말고 별도 보관하세요. 이미 배포한 앱은 내장 공개키로 서명을
확인하므로 이후 릴리스도 같은 키로 서명해야 합니다.

릴리스 태그 `vX.Y.Z`와 루트 `package.json`, 데스크톱 `package.json`, `tauri.conf.json`,
Rust 워크스페이스 버전을 일치시키면 릴리스 워크플로가 다음을 수행합니다.

1. 임시 파일 서명·검증으로 개인키·공개키·비밀번호 조합을 확인한 뒤 `pnpm build:release` 실행
2. macOS(Apple Silicon)·Windows(x64)·Linux(x64)의 설치 파일 서명과 서명된 버전을 검증하고 `latest.json` 생성
3. 모든 파일을 초안 릴리스에 업로드한 뒤 릴리스 공개

키가 없거나 키 쌍·비밀번호가 틀리거나, 버전·서명·업로드 파일이 일치하지 않으면 배포 작업이
실패합니다. 서명 검증은 업로드용 매니페스트 생성 시와 업로드 자료를 합칠 때 각각 수행합니다.
사전 릴리스 태그는 GitHub
prerelease로 발행하여 안정 버전 자동 확인 대상에서 제외합니다. 로컬 서명 빌드도 같은
환경변수와 `GITHUB_REF_NAME=vX.Y.Z`를 지정하고 `pnpm build:release`로 실행할 수 있습니다.
일반 개발·로컬 빌드는 기존 `pnpm dev`와 `pnpm build`를 사용합니다.
정식 태그를 게시하기 전에는 GitHub Actions의 `release` 워크플로를 수동 실행하고
`tag`에 예정된 태그를 입력해 서명·패키징·매니페스트를 검증할 수 있습니다.
수동 실행은 릴리스를 공개하지 않으며, 실제 공개는 태그 푸시로 진행합니다.

## 개발 환경

- Rust stable
- Node.js 22 이상
- pnpm 11
- macOS: Xcode Command Line Tools
- Linux: Tauri용 WebKitGTK 4.1 및 플랫폼 개발 라이브러리

## 개발 실행

```sh
pnpm install
pnpm dev
```

처음 실행하면 **설정 → 검증된 엔진 설치**에서 `yt-dlp`, Deno, FFmpeg, `ffprobe`를
설치하세요. 시스템에 이미 설치된 도구도 감지합니다.

데스크톱 프런트엔드 빌드:

```sh
pnpm build
```

릴리스 패키지는 `target/release/bundle/`에 생성됩니다. 코드 서명과 공증에는 배포자의
인증서가 필요하며, 이 저장소에는 서명 키가 포함되어 있지 않습니다.

## CLI와 Linux 서버

GUI 의존성 없이 CLI를 빌드할 수 있습니다.

```sh
cargo build --release -p ytlr
./target/release/ytlr tools install
./target/release/ytlr record 'https://www.youtube.com/watch?v=VIDEO_ID'
./target/release/ytlr record 'https://www.youtube.com/watch?v=VIDEO_ID' --from-now
./target/release/ytlr record 'https://www.youtube.com/watch?v=VIDEO_ID' --max-height 720
./target/release/ytlr record 'https://www.youtube.com/watch?v=VIDEO_ID' --audio-only
./target/release/ytlr record 'https://www.youtube.com/watch?v=VIDEO_ID' --for-minutes 120
./target/release/ytlr watch 'https://www.youtube.com/@CHANNEL'
./target/release/ytlr watch 'https://www.youtube.com/@CHANNEL' --audio-only
./target/release/ytlr channel configure CHANNEL_ID --max-height 480 --from-now
./target/release/ytlr status --json
./target/release/ytlr stop JOB_ID
./target/release/ytlr recover JOB_ID
./target/release/ytlr export JOB_ID --index 0
./target/release/ytlr events JOB_ID
./target/release/ytlr doctor
./target/release/ytlr schedule JOB_ID --stop-at '2026-10-01T23:30:00+09:00'
./target/release/ytlr schedule JOB_ID --clear
./target/release/ytlr bookmark add JOB_ID '공연 시작' --note '첫 곡'
./target/release/ytlr bookmark list JOB_ID
./target/release/ytlr storage JOB_ID
./target/release/ytlr cleanup JOB_ID
# 출력된 정리 계획과 파일 목록을 확인한 뒤 실행:
./target/release/ytlr cleanup JOB_ID --plan PLAN_ID --file attempt-0001/part-000000.mkv
```

대부분의 명령은 필요할 때 서비스를 백그라운드로 시작합니다. systemd나 launchd로
관리하려면 `ytlr run --headless`로 포그라운드에서 실행하세요.
`ytlr shutdown`은 진행 중인 처리를 정리하고 서비스를 종료합니다.
사용자가 직접 중지하지 않은 작업은 다음 서비스 실행 때 재개할 수 있습니다.

`--data-dir PATH` 또는 `YTLR_HOME`을 지정하면 데이터와 서비스를 별도 인스턴스로
분리해 실행할 수 있습니다. macOS 앱에 포함된 CLI는 `YTLR.app/Contents/MacOS/ytlr`에 있습니다.

## 녹화 방식

기본 설정은 **영상 + 음성 / 최고 화질**입니다. 데스크톱의 녹화 추가 화면에서
**최대 화질**을 1080p, 720p, 480p로 설정하면 해당 해상도 이하에서 가장 좋은 스트림을
선택합니다. 영상을 축소 변환하는 방식은 아닙니다. 조건에 맞는 스트림이 없으면 오류를
표시하고 재시도하며, 설정한 해상도를 임의로 초과하지 않습니다.
선택된 스트림의 해상도와 형식은 녹화 카드와 상세 화면에서 확인할 수 있습니다.

**음성만** 모드는 음성 전용 스트림이 있을 때 사용할 수 있습니다. 영상은 내려받지 않고
원본 음성 코덱을 유지합니다. 복구와 검사도 음성 기준으로 진행하며, 내보내면 재인코딩 없이
별도의 `.mka` 파일을 만듭니다. 영상 내보내기 형식은 MP4입니다.
음성만 모드와 최대 영상 해상도 제한은 함께 사용할 수 없습니다.

채널의 녹화 옵션은 새로 생성되는 작업에만 적용됩니다. 대기 중이거나 녹화 중인 작업에는
영향을 주지 않습니다. `channel configure`는 채널의 기본 녹화 옵션 전체를 교체합니다.
화질·모드 옵션을 생략하면 최고 화질 영상으로 돌아가고, `--from-now`와 `--from-start`를
모두 생략하면 전체 설정의 녹화 시작 지점을 따릅니다. 원격 이중 녹화에도 같은 옵션이 전달됩니다.

기본적으로 현재 시점부터 녹화합니다. **가능하면 방송 처음부터 저장**을 켜면 yt-dlp의
실험적 기능인 `--live-from-start`를 사용합니다. 과거 구간은 방송 플랫폼에서 아직
제공하는 범위까지만 받을 수 있습니다.

재연결 후의 녹화는 이전 시도와 따로 보관하며, 연결 지점이 불확실한 구간을 자동으로
이어 붙이지 않습니다. 사용자가 중지한 녹화는 받은 파일이 보존되어 있으면 **저장됨**으로
표시합니다. **부분 보관**은 미디어가 누락되었거나 구간이 끊김 없이 이어지는지 확인할 수
없을 때 표시합니다. 서비스가 중단되면 원본을 보존하고 작업을 재개 대기 상태로 둡니다.

완료 검사는 컨테이너, 영상·음성 헤더, 미디어 길이를 확인합니다.
방송 전체를 빠짐없이 받았다는 의미는 아닙니다.

## 저장과 복구

### 종료 예약과 북마크

녹화를 추가할 때 종료를 예약하거나, 작업 상세 화면에서 예약을 변경·취소할 수 있습니다.
**지금부터 N분 뒤** 또는 기기의 현지 시간대 기준으로 특정 종료 시각을 지정하세요.
남은 시간에는 방송 시작을 기다리는 시간도 포함됩니다.

서비스는 종료 시각을 절대 시각으로 저장하고 2초마다 확인합니다. 시간이 되면 일반 중지와
동일하게 녹화를 멈추고 파일 마무리 작업을 진행합니다. 서비스가 꺼진 사이 종료 시각이
지났다면 재시작할 때 처리하며, 기한이 지난 대기 작업은 녹화를 시작하지 않습니다.
녹화가 멈춘 뒤에도 파일 처리는 더 걸릴 수 있습니다. 작업을 수동으로 재시도하거나 복구하면
기존 종료 예약은 해제됩니다. 원격 이중 녹화에도 종료 시각이 전달되며, 변경 사항은
다음 원격 상태 확인 때 반영됩니다.

방송을 수신하는 동안 **지금 표시**를 누르면 북마크를 남길 수 있습니다. 제목과 메모는 작업 상세
화면에서 편집합니다. 북마크는 작업과 함께 저장되며, 재연결 후에도 어느 녹화 시도에서
만들었는지 유지됩니다. FFmpeg 녹화에서는 마지막으로 보고된 미디어 위치를 기록하므로
실제 위치와 조금 다를 수 있습니다. yt-dlp의 내장 수집 방식(native)으로 처음부터 녹화할 때는
실제 시각과 수신 시각을 기록하고, 알 수 없는 미디어 위치는 비워 둡니다.
북마크에서 파일을 열면 해당 녹화 시도의 결과 파일이 열립니다. 외부 플레이어에서 북마크 위치로
자동 이동하지는 않습니다. CLI에서는 `bookmark edit`과 `bookmark remove`도 사용할 수 있습니다.

### 녹화 상태 알림

서비스는 수신 중단, 반복 재연결(재시도 3회 이상), 녹화·복구 실패, 원격 이중 녹화 실패를
5초마다 확인합니다. 같은 문제를 중복 기록하지 않고, 문제가 해소되면 그 상태도 기록합니다.
재시도만으로 수신 문제가 해결된 것으로 보지는 않습니다. 새 미디어를 받거나 작업이
종료되어야 해당 알림이 해제됩니다.

데스크톱 앱은 현재 발생 중인 문제를 표시하고, 알림이 켜져 있고 권한이 허용된 경우
문제 발생·해제 알림을 보냅니다. 시스템 알림을 받으려면 데스크톱 앱이 실행 중이어야 합니다.
앱을 닫아도 서비스의 문제 기록과 이벤트는 남습니다.

### 원본 보존과 저장 공간

각 작업은 별도 폴더에 저장됩니다.

```text
recordings/VIDEO_ID_JOB_ID/
├── attempt-0001/
│   ├── session.json
│   ├── durable-fragments.jsonl
│   ├── segments.csv / source.*-FragN
│   ├── part-*.mkv / source.*
│   └── result.json
├── attempt-0002/
└── recording.json
```

SQLite는 WAL 모드와 전체 동기화를 사용합니다. 원본 조각, 분할 파일, 최종 결과를 보존해
이전 녹화 시도를 덮어쓰지 않고 복구할 수 있습니다. 이 때문에 최종 영상 크기의
2~3배에 해당하는 저장 공간이 필요할 수 있습니다.

저장 공간 상태는 현재 설정된 저장 경로와 아직 끝나지 않은 작업의 저장 경로를 대상으로
10초마다 갱신합니다. 남은 녹화 시간은 약 30초간의 기록 속도와 설정된 최소 여유 공간을
제외한 용량으로 계산합니다. 보수적인 추정을 위해 서로 다른 디스크에 저장하더라도
동시 녹화 중인 모든 작업의 기록 속도를 합산합니다. 측정값이 없거나 기록이 멈춘 동안은
예상 시간을 표시하지 않습니다. 병합, 복구, 내보내기, 다른 앱의 사용량에 따라 추가 공간이
필요할 수 있습니다.

여유 공간이 5 GiB와 설정된 최소 여유 공간의 두 배 중 더 큰 값에 도달하거나,
최소 여유 공간까지 남은 시간이 30분 이하로 예상되면 경고합니다.
데스크톱에 경고를 계속 표시하고, 알림 설정과 권한이 허용되면 시스템 알림도 보냅니다.
서비스는 영향을 받는 미완료 작업에 경고 이벤트를 기록합니다. 여유 공간이 설정한
최솟값보다 적으면 녹화를 시작하거나 계속할 수 없습니다.

**원본 복구**는 남아 있는 원본 트랙이나 저장이 끝난 분할 파일에서 재생 가능한 결과를
만듭니다. 네트워크에서 받지 못한 구간까지 되살리는 기능은 아닙니다.

### 보관함 정리

작업 상세 화면에서 원본 트랙, 조각, 분할 파일, 검증된 결과, 내보낸 파일, 메타데이터가
차지하는 용량을 확인할 수 있습니다. **정리 미리보기**를 누르면 삭제할 수 있는 파일과
보존할 결과가 표시됩니다. 파일을 선택하고 삭제를 확인하면 정리가 진행됩니다.
CLI에서는 `cleanup JOB_ID --plan PLAN_ID --file RELATIVE_PATH`를 사용합니다.
여러 파일은 `--file`을 반복해서 지정하고, 미리보기의 모든 파일을 선택하려면 `--all`을 사용하세요.

정리 대상은 완료·중지된 작업 중 누락이나 구간 연결의 불확실성이 없는 작업으로 제한됩니다.
검증된 병합 결과에 포함되어 있고 저장이 끝난 FFmpeg 분할 파일만 삭제합니다.
yt-dlp가 직접 수집한 원본 조각, 미완성 원본, 검증된 결과, 내보낸 파일은 보존합니다.
삭제 전 결과 파일의 해시, 미디어 트랙과 길이, 원본 기록, 삭제 대상의 해시를 다시 확인합니다.
미리보기 이후 내용이 달라졌거나 오래된 계획이면 실행하지 않습니다.

큰 녹화 파일은 해시 확인에 시간이 걸릴 수 있습니다. 같은 작업을 정리하는 동안에는
파일 마무리나 백업이 동시에 진행되지 않습니다. 클라이언트 연결이 끊겨도 정리는 계속됩니다.

정리 내역과 변경 전 원본 기록은 보관합니다. 선택한 파일을 지우기 전에 현재 원본 기록을
갱신하므로, 이후 복구나 백업에서 의도적으로 삭제한 파일을 손상으로 판단하지 않습니다.
이 기록도 백업되며 기존 백업 파일은 삭제하지 않습니다. 정리가 도중에 중단되면 남은 파일은
그대로 두고 새 미리보기를 만들어 다시 진행할 수 있습니다.

## 자동화와 보관함

이 기능은 서비스 API 3을 사용합니다. 이전 서비스가 실행 중이면 녹화를 마무리하고
`ytlr shutdown` 후 새 버전으로 시작하세요. 원격 서비스도 함께 업데이트해야 합니다.

### 채널 규칙과 주간 반복 녹화

**채널 → 녹화 옵션 → 자동 녹화 규칙**에서 포함·제외 키워드를 한 줄에 하나씩 입력합니다.
대소문자는 구분하지 않으며 포함 키워드는 하나 이상 일치하면 허용합니다. 제외 키워드가
우선하고, 포함 목록이 비어 있으면 모든 제목을 허용합니다. 미리보기에 제목과 시각을
입력하면 저장 전 판단을 확인할 수 있습니다. 채널 옵션에 최근 20개 판단이 남습니다.

주간 반복 시간대를 켜면 선택한 요일·시간대에서만 방송을 감지하여 작업을 만듭니다.
종료가 시작보다 이르거나 같으면 다음 날 종료합니다(같은 시각은 24시간).
요일은 **시작일 기준**, 시간대는 **고정 UTC 오프셋(한국: 540분)**입니다.
일광 절약 시간 변경은 직접 반영해야 합니다. 시간대가 끝나면 해당 회차 녹화도 중지합니다.
서비스가 꺼져 있던 회차는 소급 실행하지 않고, 현재 시간대에 남은 구간만 녹화합니다.
시작 감지에는 설정된 채널 감시 주기만큼 지연이 생길 수 있습니다.

방송·채널·회차별 실행 기록은 작업 삭제 후에도 남습니다. 재시작, 수동 중지, 보관 기간
삭제로 같은 회차가 중복 생성되지 않습니다. 다음 회차에는 같은 상시 방송을 다시 녹화할 수
있습니다. 규칙 변경은 이미 생성된 작업에는 소급 적용하지 않습니다.

```sh
ytlr channel rules CHANNEL_ID
ytlr channel rules CHANNEL_ID --file rules.json --preview-title '오늘의 공연' --at '2026-10-05T23:30:00+09:00'
ytlr channel rules CHANNEL_ID --file rules.json
```

`rules.json` 예시(파일은 CLI를 실행하는 기기에서 읽습니다):

```json
{
  "include": ["공연", "concert"],
  "exclude": ["test", "재방송"],
  "window": {
    "weekdays": [1, 2, 3, 4, 5],
    "start_minute": 1380,
    "end_minute": 60,
    "utc_offset_minutes": 540
  },
  "duration_minutes": 90
}
```

### 시작 시각과 녹화 길이

녹화 추가 화면에서 시작 시각과 **실제 녹화 시작 후 최대 분**을 지정할 수 있습니다.
시작 전 작업의 상세 화면에서 예약을 변경하거나 비워서 취소할 수 있습니다.
녹화 시간 제한은 첫 수집 시도 시작부터 계산하며 재연결·재시작으로 연장되지 않습니다.
종료 예약과 함께 지정하면 더 이른 종료 시각을 적용합니다. 기존 **지금부터 N분 뒤**는
여전히 방송 대기 시간을 포함합니다. 수동 재시도·복구는 시작·종료 예약과 시간 제한을 해제합니다.

```sh
ytlr record 'https://youtu.be/VIDEO_ID' --start-at '2026-10-01T20:00:00+09:00' --duration-minutes 120
ytlr start-schedule JOB_ID --start-at '2026-10-01T21:00:00+09:00' --duration-minutes 60
ytlr start-schedule JOB_ID  # 시작 예약과 녹화 시간 제한 해제
```

### 서비스 외부 알림

**설정 → 자동화와 보관 정책**에서 Webhook·Discord·Telegram 대상을 추가합니다.
서비스는 녹화 시작·완료, 문제 발생·해제, 저장 공간 경고 등의 이벤트를 DB와 함께 저장하고
앱 실행 여부와 관계없이 전송합니다. 실패하면 지수 백오프(최대 1시간 간격)로 재시도하며,
서비스 재시작 후에도 전송 대기 목록이 유지됩니다. 전송 성공 직후 서비스가 중단되면
같은 알림이 다시 전달될 수 있습니다. 일반 Webhook 수신자는 `delivery_id`로 중복 제거할 수 있습니다.

URL과 토큰은 설정에 직접 저장하지 않고 **서비스 프로세스의 환경변수 이름**으로 참조합니다.
systemd/launchd 또는 서비스를 실행하는 셸에 실제 값을 설정한 뒤 서비스를 시작하세요.
이미 실행 중인 서비스에는 셸의 환경변수 변경이 전달되지 않습니다. 원격 대상은 원격
서비스의 환경변수를 사용합니다. HTTPS를 사용하며 로컬 테스트에는 localhost HTTP도 허용합니다.

`automation.json` 예시:

```json
{
  "notifications": [
    { "id": "ops", "kind": "webhook", "url_env": "YTLR_WEBHOOK_URL" },
    { "id": "discord", "kind": "discord", "url_env": "YTLR_DISCORD_URL" },
    { "id": "telegram", "kind": "telegram", "token_env": "YTLR_TELEGRAM_TOKEN", "chat_id": "CHAT_ID" }
  ],
  "retention": { "cleanup_after_days": 7, "delete_after_days": null }
}
```

```sh
ytlr automation --file automation.json  # 자동화 설정 전체 교체
ytlr notifications
ytlr notifications --retry DELIVERY_ID
ytlr maintenance
```

일반 Webhook은 `{ "delivery_id": 1, "event": { "job_id": "…", "kind": "…", "at": "…", "message": "…" } }`
형식의 JSON을 POST합니다. UI와 CLI에서 최근 전송 내역과 실패 이유를 확인할 수 있습니다.
대상 ID를 삭제하면 해당 대기 알림은 실패 상태로 유지되며 같은 ID를 다시 등록하면 재시도합니다.

### 보관함 필터·일괄 처리·클립

보관함에서 제목·채널 검색, 채널·상태·날짜·중요 녹화 필터와 최신·오래된·용량·제목 정렬을
사용할 수 있습니다. 날짜는 이 기기의 현지 날짜이며 끝 날짜 전체를 포함합니다.
**표시된 작업 전체 선택**은 현재 필터 결과만 선택합니다. 선택 내보내기는 각 작업의 모든
결과 파일을 순서대로 처리하고, 실패해도 다음 작업을 계속하며 작업별 결과를 표시합니다.
일괄 정리는 미리보기의 파일 목록을 확인한 뒤 실행하며 기존 해시·미디어 재검증을 적용합니다.

작업 상세의 **북마크 클립 내보내기**에서는 북마크 전후 N초 또는 두 북마크 사이를
별도 MP4(음성 전용은 MKA)로 저장합니다. 재인코딩하지 않아 키프레임에 따라 경계가 조금
달라질 수 있습니다. 위치 미확인 북마크, 역순 구간, 재연결 시도 간 구간은 사용할 수 없습니다.

```sh
ytlr library --search 공연 --state completed --sort size
ytlr library --channel '채널 이름' --from '2026-10-01T00:00:00+09:00' --until '2026-11-01T00:00:00+09:00' --export
ytlr library --protected --cleanup-preview
ytlr clip JOB_ID BOOKMARK_ID --before 30 --after 60
ytlr clip JOB_ID START_BOOKMARK_ID --end-bookmark END_BOOKMARK_ID
```

### 보관 정책과 중요 녹화 보호

자동 정리·삭제는 기본적으로 꺼져 있습니다. 설정에서 일수를 지정하면 서비스 시작 시와
매시간 녹화 종료일을 기준으로 처리합니다. 이전 버전의 종료 작업은 마지막 갱신 시각을
종료일 대신 사용합니다. **검증된 분할 원본 정리**는 기존 정리 기준을 만족하는 파일만
삭제하고 결과·내보낸 파일을 남깁니다. **녹화 전체 삭제**는 만료된 작업 폴더와 작업을 삭제합니다.
두 조건이 함께 충족되면 전체 삭제를 적용합니다. 별도 백업과 원격 녹화는 삭제하지 않습니다.

보관함의 **중요 녹화 보호**를 켜면 수동·자동 정리와 전체 삭제를 차단합니다.
내보내기와 클립 생성은 가능합니다. 실행 중·백업 중인 작업은 보류하며 실패 기록을 남깁니다.
파일 삭제가 실패하면 작업 기록을 남겨 재시도할 수 있습니다. 자동 처리 내역은 작업 삭제 후에도
설정의 자동 정리 내역과 `ytlr maintenance`에 남습니다.

```sh
ytlr protect JOB_ID
ytlr protect JOB_ID --clear
```

## 백업과 원격 제어

백업할 폴더는 미리 만들어 두어야 합니다. YTLR은 대상 볼륨을 식별하고, 모든 사본을
검증한 뒤에만 백업 완료 목록을 확정합니다. 파일 크기가 같아도 해시가 다르면 다시 복사합니다.
백업에 실패해도 녹화는 계속됩니다.

원격 제어는 SSH 터널을 사용하며 서버 API는 localhost에서만 연결을 받습니다.
CLI에서 원격 서버를 등록할 수 있습니다.

```sh
ytlr remote add studio \
  --ssh recorder@server \
  --remote-data-dir /home/recorder/ytlr-data \
  --executable /home/recorder/.local/bin/ytlr
ytlr --remote studio status
```

SSH 키 인증과 신뢰할 호스트 키를 먼저 설정해야 합니다.
원격 서버를 이중 녹화 대상으로 지정할 수 있지만, 두 녹화 결과를 자동으로 합치지는 않습니다.

## 테스트

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
pnpm check
pnpm test
pnpm test:integration
pnpm test:ui
pnpm test:remote
```

로컬 HLS 스트림에 장애를 발생시켜 복구와 재시작 동작을 확인하는 장시간 테스트도 있습니다.

```sh
pnpm test:soak --seconds 600
pnpm test:soak --seconds 86400
```

CI에서는 Linux와 Windows의 코어·GUI 빌드를 확인합니다. 실제 공개 라이브 방송과의 호환성,
24~72시간 연속 운전, 정전, 물리적 디스크 고장, 플랫폼별 서명은 별도의 실환경 검증이 필요합니다.

## 라이선스

프로젝트 코드는 MIT 라이선스를 따릅니다. 함께 배포하는 외부 도구에는 각 도구의 라이선스가
적용됩니다. 버전과 출처는 [THIRD_PARTY.md](THIRD_PARTY.md)를 참고하세요.
