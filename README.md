# Grabber

yt-dlp 기반 윈도우용 유튜브 다운로더. 링크를 붙여넣고 버튼 한 번이면 영상, 자동 자막, 썸네일을 함께 받습니다.

## 설치

[Releases](../../releases/latest)에서 `Grabber_x.x.x_x64-setup.exe`를 받아 실행합니다. 사용자 폴더에 설치되어 관리자 권한이 필요 없어요.
처음 실행하면 yt-dlp, ffmpeg, Deno를 자동으로 받습니다.

서명되지 않은 앱이라 처음 설치할 때 SmartScreen 경고가 뜨면 "추가 정보 → 실행"을 누르세요. 이후 업데이트는 앱이 직접 받기 때문에 경고가 다시 뜨지 않습니다.

## 업데이트

- **다운로드 엔진** (yt-dlp · Deno · ffmpeg): 하루 한 번 실행할 때 자동으로 확인합니다. 설정 화면의 "지금 업데이트"로 바로 받을 수도 있어요. 유튜브 변경으로 다운로드가 막히면 업데이트 채널을 "나이틀리"로 바꿔보세요.
- **앱 자체**: 새 버전이 올라오면 아래 상태 표시줄에 "새 버전 설치"가 나타납니다.

## 새 버전 배포

1. `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `package.json`의 버전을 올립니다.
2. `release` 브랜치에 푸시하거나 GitHub Actions의 **Release** 워크플로를 실행합니다.
3. 설치 파일과 `latest.json`이 담긴 릴리스가 만들어지고, 설치된 앱이 이를 감지합니다.

릴리스 빌드에는 저장소 Secret `TAURI_SIGNING_PRIVATE_KEY`(업데이트 서명 키)가 필요합니다.

## 구조

- `src/` — 화면 (HTML/CSS/JS, 프레임워크 없음)
- `src-tauri/src/logic.rs` — yt-dlp 정보 해석, 명령줄 구성, 진행률 해석 (단위 테스트 포함)
- `src-tauri/src/download.rs` — 영상 정보 조회, 다운로드 실행·취소
- `src-tauri/src/tools.rs` — yt-dlp / ffmpeg / Deno 설치와 업데이트
