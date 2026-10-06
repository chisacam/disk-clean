# disk-clean

디스크를 무엇이 차지하는지 — macOS 의 '시스템 데이터'·'문서'처럼 뭉뚱그려지는 용량까지 — 재서 보여 주고,
**다시 만들 수 있는 것만** 골라 지우는 CLI. macOS 와 Linux 용.

```sh
cargo install --path .     # ~/.cargo/bin/disk-clean

disk-clean                 # = scan. 읽기만 함
disk-clean top ~ -d 2      # 큰 폴더·파일 순위 (정체불명 찾기)
disk-clean clean           # 터미널에서 골라 지우기 (확인 후)
disk-clean clean caches dev:.terraform --dry-run

disk-clean plan caches/Homebrew npm   # 지울 계획만 만들어 저장 (1시간, 한 번)
disk-clean apply 3fa2c19b04d1         # 그 계획에 적힌 경로만 다시 확인하고 지움
disk-clean log                        # 지운 기록
disk-clean whitelist add ~/Library/Logs/mole   # 절대 지우지 않을 경로
```

`scan`·`top`·`plan`·`apply`·`log` 는 `--json` 을 받는다. 크기는 바이트 정수, 오류도 JSON 이다.
종료 코드: 0 성공 · 1 오류 · 2 일부만 지움 · 3 거부(안전장치가 막음).

## 지원 플랫폼

| 플랫폼 | 상태 | 확인한 방법 |
|---|---|---|
| macOS (Apple Silicon) | 지원 | macOS 26 에서 개발·실사용, `tests/e2e.py`, CI `macos-latest` |
| Linux (x86_64) | 지원 | CI `ubuntu-latest` 에서 테스트·`tests/e2e.py`·실제 홈 scan |
| macOS (Intel), Linux (arm64) | 빌드 대상이지만 시험하지 않음 | — |
| Windows | 지원하지 않음 | 빌드가 오류 한 줄로 멈춘다 |

측정·개발 산출물·plan/apply·감사 로그는 두 플랫폼에서 같다. 플랫폼마다 다른 것은 어디를 보는가다.

| | 공통 | macOS | Linux |
|---|---|---|---|
| 안전 | npm·uv·bun 캐시 | `~/Library/Caches`(`com.apple.*` 제외), 로그, Xcode DerivedData, 메일 첨부 사본, 휴지통 | `~/.cache`(AI 모델·uv 제외), 휴지통(`~/.local/share/Trash`) |
| 재생성 가능 | Cargo·Gradle·Maven·Go·Terraform 캐시, 프로젝트의 `node_modules`·`target`·`.terraform`·`.venv` … | 샌드박스 앱 캐시, Xcode 기기 지원 파일, 시뮬레이터 캐시, pnpm | Flatpak 앱 캐시(`~/.var/app/*/cache`), pnpm |
| 지운 앱이 남긴 데이터 | — | 앱이 없어진 컨테이너와 그 앱의 `/var/folders` 캐시 | — |
| 직접 판단 (지우지 않음) | 로컬 AI 모델, `~/Downloads` | 앱 컨테이너·그룹 컨테이너, Application Support, Xcode Archives, 시뮬레이터, 메시지 첨부 | `~/.local/share`, `~/.var/app` |
| 시스템 (정보만) | 디스크 전체·남은 공간 | 로컬 스냅샷, `/private/var/vm`, `/var/folders` | systemd 저널, `/tmp`, `/var/tmp`, 스왑 파일 |

Linux 의 한계: 다운로드 폴더는 `~/Downloads` 만 본다(XDG 사용자 폴더 이름이 '다운로드'처럼 현지화돼 있으면 보지 않는다).
root 권한이 필요한 곳(systemd 저널 정리, `/var/lib/docker`, Snap)은 지우지 않고, 저널은 줄이는 명령만 안내한다.

## 분류

| 분류 | 뜻 | `clean` |
|---|---|---|
| 안전 | 앱이 알아서 다시 만듦 | 지움 (목록에서 기본 선택) |
| 재생성 가능 | 다시 받거나 빌드해야 함 | 지움 (직접 골라야 함) |
| 지운 앱이 남긴 데이터 | 앱을 지운 뒤 남은 것 — 아래 세 조건이 모두 맞을 때만 (macOS) | 지움 (직접 골라야 함, 지우기 직전 재확인) |
| 직접 판단 | 사용자 데이터일 수 있음 | **거부** — 앱 안에서 정리 |
| 시스템 | 운영체제가 관리 | 정보만 |

개발 산출물은 이름만으로 고르지 않는다. 옆에 프로젝트 파일이 있어야 한다
(`node_modules` ↔ `package.json`, `target` ↔ `Cargo.toml`/`pom.xml`/`CACHEDIR.TAG`, `.venv` ↔ `pyvenv.cfg` …).
`.terraform` 은 `.tf` 가 있거나, 코드가 지워진 뒤 남은 고아(`providers`·`modules` 가 든 것)도 잡는다.
모듈 clone 안에 중첩된 `.terraform` 은 따로 경고한다.

앱이 지워졌다는 판정은 셋이 모두 맞아야 한다: 컨테이너 메타데이터에 기록된 앱 경로가 없고,
LaunchServices 에 그 bundle id 로 등록된 앱이 없고, Spotlight 에서도 찾을 수 없을 것.
외장 디스크(`/Volumes/…`)에 있던 앱은 꽂혀 있지 않은 것과 구별이 안 되므로 판정하지 않는다.

## whitelist

[Mole](https://github.com/tw93/Mole) 의 whitelist 와 같은 방식이다. `~/.config/disk-clean/whitelist`
(`$XDG_CONFIG_HOME` 이 있으면 그 아래)에 한 줄에 하나씩 적는다. `#` 은 주석, `~`·`$HOME` 을 쓸 수 있고 `* ? [...]` glob 이 된다.

```sh
disk-clean whitelist                        # 목록
disk-clean whitelist add ~/Library/Logs/mole '~/Library/Caches/JetBrains*'
disk-clean whitelist remove ~/Library/Logs/mole
```

- 지울 경로가 패턴과 같거나 glob 에 맞으면, 보호 폴더의 안쪽이면, 보호 경로를 품은 상위 폴더면 지우지 않는다.
  항목은 폴더째 지워지므로 안쪽에 하나라도 보호할 것이 있으면 그 폴더 전체를 남긴다.
- scan 은 보호된 경로를 후보에서 빼고 몇 개를 뺐는지 보여 준다. 삭제 직전 검사도 whitelist 를 다시 읽으므로,
  계획을 만든 뒤 추가한 경로도 apply 가 거부한다.
- 잘못 적은 줄(상대 경로, `..`, `//`, `/` 전체)은 쓰지 않고 경고한다.
- **기본 보호**는 파일과 관계없이 항상 켜져 있고 뺄 수 없다. 지우면 다시 만드는 비용이 아니라 고장이 나는 것들로,
  Mole 의 필수 보호 목록에서 가져왔다: macOS 는 `~/Library/Caches/CloudKit*`(iCloud 동기화), Poetry 가상 환경, renv 캐시,
  Linux 는 `~/.cache/pypoetry/virtualenvs*`, `~/.cache/R/renv*`.
- 에이전트: `whitelist`·`whitelist add` 는 지울 수 있는 범위를 줄이기만 하므로 자유롭게, `whitelist remove` 는 범위를 넓히므로
  `apply` 처럼 승인을 받는다(Claude Code `ask` 규칙, pi 확장).

## 안전장치

- `scan`·`top`·`--dry-run` 은 아무것도 바꾸지 않는다.
- 지우기 전에 항목·경로·크기를 보여 주고 `y` 를 받는다. 터미널이 아니면 `clean` 은 지우지 않는다 — `plan` 과 `apply` 를 쓴다.
- 홈 폴더 밖, 홈 바로 아래, 보호 목록(`~/Library/Caches`·`~/.cache`·`~/.local/share` 자체, `~/Documents` …), 상위 경로에 심볼릭 링크가 낀 곳은 지우지 않는다.
- 심볼릭 링크는 따라가지 않고, 다른 볼륨으로 넘어가지 않는다. `com.apple.*` 캐시는 건드리지 않는다.
- 개발 산출물은 지우기 직전에 표식을 다시 확인한다.
- 지운 뒤 늘어난 공간은 추정이 아니라 `statfs` 로 잰 값을 보여 준다.

## 에이전트가 쓸 때

셸을 가진 에이전트는 이 도구를 거치지 않고도 `rm` 할 수 있다. 그래서 승인 관문은 하네스에 두고,
도구는 사람이 승인할 대상을 분명하게 만든다.

- **plan / apply.** 에이전트는 `plan` 으로 계획을 만들어 보여 주고, 사람이 그 계획 ID 를 승인하면 `apply` 한다.
  `clean` 은 터미널이 아니면 지우지 않는다.
- **apply 는 계획 파일을 믿지 않는다.** 계획 파일은 누구나 고칠 수 있으므로 apply 는
  ① ID 가 내용의 해시와 같은지, ② 지금 다시 잰 scan 에서도 같은 분류로 지울 수 있는 경로인지,
  ③ 같은 파일·폴더(device·inode)이고 계획보다 크게 커지지 않았는지(10% 와 64 MiB 중 큰 쪽)를 경로마다 본다.
  계획은 1시간 뒤 만료되고, 성공하든 거부되든 한 번 쓰면 끝난다.
- **감사 로그.** 지울 때마다 `~/.local/state/disk-clean/audit.jsonl` 에 한 줄을 남긴다
  (시각, 누가 — `claude-code`·`pi:<세션>`·`terminal`, 겹쳐 띄웠으면 표식 전부, 계획, 지운 경로와 크기, 거부·실패, 전후 남은 공간).
  `~/Library/Logs` 에 두지 않는 것은 `logs` 규칙이 그 폴더를 비우기 때문이다.

하네스 연결 (원본은 `integrations/`, 심볼릭 링크로 설치):

```sh
# 사용법 skill — Claude Code 와 pi
ln -s "$PWD/integrations/skill/disk-clean" ~/.claude/skills/disk-clean
ln -s "$PWD/integrations/skill/disk-clean" ~/.pi/agent/skills/disk-clean
# pi: apply·whitelist remove 를 가로채 내용을 보여 주고 승인을 받는 확장. clean 은 막는다
ln -s "$PWD/integrations/pi/disk-clean-gate" ~/.pi/agent/extensions/disk-clean-gate
```

Claude Code 는 `~/.claude/settings.json` 의 권한 규칙으로 막는다:

```json
"permissions": {
  "allow": ["Bash(disk-clean scan:*)", "Bash(disk-clean top:*)", "Bash(disk-clean plan:*)", "Bash(disk-clean log:*)",
            "Bash(disk-clean whitelist:*)"],
  "ask":   ["Bash(disk-clean apply:*)", "Bash(disk-clean whitelist remove:*)", "Edit(~/.config/disk-clean/**)"],
  "deny":  ["Bash(disk-clean clean:*)"]
}
```

`Edit(~/.config/disk-clean/**)` 은 에이전트가 whitelist 파일을 편집 도구로 직접 고쳐 `remove` 승인을 건너뛰는 길을 막는다.
셸(`echo >>`)로 고치는 길은 규칙으로 막을 수 없어 skill 의 지시로만 막는다.

Claude Code 의 규칙은 명령 앞부분으로 맞추므로 `~/.cargo/bin/disk-clean apply …` 처럼 전체 경로로 부르면
`ask` 에 걸리지 않는다(skill 이 이름으로만 부르게 한다). pi 확장은 전체 경로 형태도 잡는다.

## 측정

- 크기는 할당된 블록(`st_blocks`) 기준이라 sparse 파일이 부풀지 않는다. 하드 링크는 한 번만 센다.
  APFS 복제본(clone)은 구분하지 못해 실제보다 크게 잡힐 수 있다.
- 단위는 GiB(1024³). macOS 설정 앱은 GB(1000³)라 7% 정도 크게 표시된다.
- macOS 는 다른 앱의 컨테이너를 읽는 시스템 호출을 가끔 중단시킨다(EINTR, 어디서든 생길 수 있어 공통으로 처리한다). 읽기·삭제는 이때 다시 시도하고,
  그래도 못 읽은 곳은 버리지 않고 "읽지 못한 곳" 으로 센다. `DISK_CLEAN_DEBUG=1` 로 경로와 오류를 볼 수 있다.
- macOS: 휴지통·메일·메시지·Safari 는 **전체 디스크 접근 권한**이 있어야 잰다. 권한이 없으면 그렇다고 표시하고,
  읽지 못한 폴더 수를 보여 준다. 권한은 이 프로그램이 아니라 실행하는 터미널 앱에 준다
  (시스템 설정 › 개인정보 보호 및 보안 › 전체 디스크 접근 권한).

## 개발

```sh
cargo clippy --all-targets -- -D warnings && cargo test
cargo build --release && python3 tests/e2e.py target/release/disk-clean   # 가짜 홈에서 scan → plan → apply
node --test integrations/pi/gate.test.ts                                   # pi 확장 (Node 23.6+)
```

CI(`.github/workflows/ci.yml`)는 이 세 가지를 `ubuntu-latest` 와 `macos-latest` 에서 돌리고, 러너의 실제 홈을 읽기만 하는 scan 도 한 번 돌린다.

## 라이선스

MIT — [LICENSE](LICENSE)
