---
name: disk-clean
description: Find out what fills the disk on macOS or Linux (on a Mac, the "시스템 데이터" or "문서" bars in Storage settings) and free space safely with the disk-clean CLI — measure, explain, plan, and delete only what the user approved. Use for "디스크 정리", "용량 확보", "저장 공간이 부족해", "시스템 데이터가 왜 이렇게 커", "캐시 지워줘", "node_modules 정리", "지운 앱 찌꺼기", "free up disk space", "what is taking space on my Mac". Not for Windows, and not for deleting documents, photos or other files the user made.
---

# disk-clean

재기·분류·계획은 마음대로 해도 된다. 삭제는 **사용자가 승인한 계획**으로만 한다.

## 절대 규칙

1. 이 작업에서 파일을 지우는 길은 `disk-clean apply <계획 ID>` 하나다. 같은 경로를 `rm`, `trash`, `find -delete`, Finder 로 지우지 않는다.
2. `disk-clean clean` 은 쓰지 않는다. 터미널 앞의 사람용이라 에이전트가 실행하면 거부된다(종료 코드 3).
3. apply 는 사용자가 **그 계획 ID** 를 보고 지워도 된다고 말한 뒤에만 실행한다. plan 직후 이어서 apply 하지 않는다. apply 때 하네스가 한 번 더 확인 창을 띄우는 것은 정상이다(Claude Code 의 `ask` 규칙, pi 의 `disk-clean-gate` 확장).
4. 계획 파일(`~/.local/state/disk-clean/plans/*.json`)을 고치거나 직접 만들지 않는다. 고친 계획은 ID 와 내용이 맞지 않아 거부된다.
5. 계획 ID 는 셸 변수로 넘기지 말고 글자 그대로 적는다(`disk-clean apply 3fa2c19b04d1`). pi 의 확인 창은 그 ID 로 계획을 읽어 사람에게 보여 주는데, `"$ID"` 는 읽을 수 없어 막는다.
6. `disk-clean` 이 PATH 에 없으면 전체 경로로 우회하지 말고 사용자에게 알린다(설치: 이 저장소에서 `cargo install --path .`). Claude Code 의 권한 규칙은 `disk-clean …` 으로 시작하는 명령에만 걸린다.
7. `review` 항목은 도구가 지우지 않는다. 앱 안에서 정리하는 방법을 안내한다.
8. whitelist 는 사용자가 요청할 때만 줄인다(`disk-clean whitelist remove`). 하네스가 확인 창을 띄운다. whitelist 파일(`~/.config/disk-clean/whitelist`)을 직접 고치지 않는다 — 그러면 그 확인을 건너뛴다.

## 순서

1. **재기** — `disk-clean scan --json`. 읽기만 하고 10초 안팎 걸린다. 이름 없는 큰 폴더는 `disk-clean top <경로> -d 2 --json` 으로 내려가 본다.
2. **설명** — 분류별로 쉬운 말로, 무엇이고 지우면 무슨 일이 생기는지. 크기는 `bytes` 를 GiB 로 바꿔 말하되 단위를 밝힌다(macOS 설정 앱은 GB 라 7% 크게 보인다). 운영체제가 관리하는 `system` 항목은 설명만 한다 — Linux 의 systemd 저널은 `sudo journalctl --vacuum-time=…` 을 사용자가 직접 실행하도록 안내하고, 대신 실행하지 않는다.
3. **고르기** — 사용자가 고른다. 먼저 권할 것은 `safe` 다. `regenerable` 과 `leftover` 는 아래 표의 비용을 말하고 묻는다.
4. **계획** — `disk-clean plan <ID…> --json`. 출력의 `id`, `items[].id`·`bytes`, `total_bytes`, `items[].notes`, `refused` 를 그대로 보여 준다. 계획은 1시간 동안, 한 번만 쓸 수 있다.
5. **승인** — 사용자가 그 계획으로 지워도 된다고 할 때까지 기다린다.
6. **실행** — `disk-clean apply <ID> --json`.
7. **보고** — `freed`(statfs 로 잰 남은 공간의 실제 증가분)와 `deleted_bytes` 를 말한다. 예상치가 아니라 이 값이다. `refused`·`failed` 가 있으면 경로와 이유를 전한다.

## 분류

| `safety` | 무엇 | 지우기 전에 말할 것 |
|---|---|---|
| `safe` | 앱이 다시 만드는 캐시·로그 | 실행 중인 앱은 끄고 지우는 편이 깔끔하다 |
| `regenerable` | 다시 받거나 빌드해야 하는 것 | 비용: `node_modules` → 다시 install, `target` → 다시 build, `.terraform` → `terraform init`(원격 backend 에 닿아야 한다 — VPN 이 필요한 환경이면 연결된 상태에서), 고른 workspace 는 default 로 돌아간다 |
| `leftover` | 이미 지운 앱이 남긴 컨테이너 (macOS 만) | 앱을 다시 설치하면 리소스·설정·로컬 세이브를 처음부터 받는다 |
| `review` | 사용자 데이터일 수 있는 것 | 도구가 지우지 않는다 |

- `unreadable` 이 0 보다 크면 그 크기는 하한값이다. macOS 에서 `full_disk_access` 가 `false` 면 휴지통·메일·메시지·Safari 는 재지 못했다. 터미널 앱에 '전체 디스크 접근 권한'을 주면 잴 수 있다고 안내한다(Linux 에는 이 권한이 없고 값은 `null`).
- 상위 ID 는 하위를 모두 포함한다. `caches` 는 `caches/Homebrew` 같은 하위 항목까지, `dev:.terraform` 은 `dev:.terraform:<프로젝트>` 까지 지운다. 일부만 원하면 하위 ID 를 쓴다.
- `notes` 의 ⚠ 표시는 그대로 전한다(모듈 clone 안에 중첩된 `.terraform`, 코드가 지워진 뒤 남은 `.terraform`, 선택된 workspace).

## whitelist

사용자가 "이건 지우지 마" 라고 하면 계획에서 빼는 것으로 끝내지 말고 whitelist 에 넣는다. 다음 정리 때도 지켜진다.

```sh
disk-clean whitelist                          # 목록 (기본 보호 + 사용자 것)
disk-clean whitelist add ~/Library/Logs/mole  # 추가 — 경로, ~ · $HOME, * ? [..] glob
disk-clean whitelist remove ~/Library/Logs/mole   # 빼기 — 사용자가 요청했을 때만
```

- 폴더를 넣으면 그 안쪽도, 그 폴더를 품은 상위 폴더도 지우지 않는다(항목은 통째로 지워지므로).
- scan JSON 의 `items[].protected` 는 whitelist 때문에 뺀 경로 수다. 크기에는 들어가지 않는다. `whitelist.warnings` 가 있으면 사용자에게 알린다(잘못 적어서 적용되지 않은 줄).
- 기본 보호(CloudKit 캐시, Poetry 가상 환경, renv 캐시 — 지우면 다시 빌드로 끝나지 않고 깨지는 것)는 뺄 수 없다.
- 계획을 만든 뒤 whitelist 에 넣은 경로도 apply 가 거부한다.

## 종료 코드와 거부

0 성공 · 1 오류 · 2 일부만 지움 · 3 거부(안전장치가 막음). `--json` 이면 오류도 JSON(`kind: "refused" | "error"`)이다.

apply 가 거부하는 경우는 이렇다. 이유를 사용자에게 전하고, 필요하면 다시 plan 한다.

- 계획 파일이 만들어진 뒤 바뀜, 만료(1시간), 이미 실행한 계획
- 다시 재 보니 더는 지울 대상이 아님(분류가 바뀌었거나 프로젝트 파일이 사라짐)
- 같은 이름의 다른 파일·폴더로 바뀜
- whitelist 에 있음(계획 뒤에 추가했어도)
- 계획보다 크게 커짐(10% 와 64 MiB 중 큰 쪽을 넘음)

## 기록

`disk-clean log --json` — 언제, 누가(`claude-code`, `pi:<세션>`, `terminal`, 하네스를 겹쳐 띄웠으면 `pi:<세션>+claude-code` 처럼 전부), 무엇을 지웠는지. 파일은 `~/.local/state/disk-clean/audit.jsonl`.
