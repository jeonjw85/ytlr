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

현재 버전은 **0.2.3**이며, 안정화 테스트를 진행하고 있습니다.

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
