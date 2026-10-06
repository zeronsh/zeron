# Zeron

코딩 에이전트(Claude Code, Codex, Cursor, Devin, Grok, Hermes, Pi, Antigravity)를 기본적으로 로컬에서 관리하고, 필요하면 여러 기기 간 동기화도 사용할 수 있습니다.

*[English](README.md) | [简体中文](README.zh-CN.md) | 한국어 | [日本語](README.ja.md)*

![Zeron 데스크톱 앱](docs/media/readme/app-screenshot.jpg)

## 데스크톱 앱

[GitHub Releases](https://github.com/zeronsh/zeron/releases/latest)에서 플랫폼에 맞는 최신 버전을 내려받으세요.

- **macOS** — `zeron-<version>-macos-arm64.dmg`
- **Windows** — `zeron-<version>-windows-x86_64-setup.exe`
- **Linux** — `zeron-<version>-linux-<arch>.tar.gz`의 압축을 풀고 안에 있는 `install.sh` 실행

계정이나 네트워크 연결이 필요 없으며, 세션은 사용 중인 기기에 저장됩니다. 앱은 자동으로 업데이트됩니다.

## 헤드리스 실행 (CLI)

서버처럼 디스플레이가 없는 머신용입니다. 예를 들어 노트북을 닫은 뒤에도 에이전트를 계속 돌려 두는 VPS에 쓰면 됩니다. Linux만 지원합니다.

```bash
curl -fsSL https://zeron.sh/install.sh | sh
zeron status
```

설치 스크립트는 엔진을 백그라운드 서비스로 실행하며, 재부팅 후에도 자동으로 다시 시작됩니다.

```bash
zeron status      # 로컬/동기화 모드와 엔진 상태 확인
zeron update      # 최신 버전으로 업데이트
zeron daemon start|stop|restart|status
```

## 여러 기기 간 동기화 (선택)

로그인하면 한 기기에서 에이전트를 시작하고 다른 기기에서 이어서 보거나 조작할 수 있습니다.

```bash
zeron daemon stop
zeron login        # 로컬 전용 모드로 돌아가려면 zeron logout
zeron daemon start
```

같은 계정에 로그인한 기기끼리는 서로의 워크스페이스 파일을 읽고 쓸 수 있으므로, 신뢰하는 기기에서만 로그인하세요. 기존 로컬 세션은 업로드되지 않습니다.

## 후원

Zeron을 후원해 주신 [The Context Company](https://www.thecontextcompany.com/)에 감사드립니다. [GitHub에서 후원자가 되어](https://github.com/sponsors/zeronsh) Zeron 개발을 지원하실 수도 있습니다.

---

개발에 참여하고 싶거나 동작 방식이 궁금하다면 [Ask DeepWiki](https://deepwiki.com/zeronsh/zeron) 또는 [ARCHITECTURE.md](ARCHITECTURE.md)를 확인하세요.

[MIT License](LICENSE)로 배포됩니다.
