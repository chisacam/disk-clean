# disk-clean

macOS 의 '시스템 데이터'·'문서'로 뭉뚱그려지는 용량이 실제로 어디에 있는지 재서 보여 주고,
**다시 만들 수 있는 것만** 골라 지우는 CLI.

```sh
cargo install --path .     # ~/.cargo/bin/disk-clean

disk-clean                 # = scan. 읽기만 함
disk-clean top ~ -d 2      # 큰 폴더·파일 순위 (정체불명 찾기)
disk-clean clean           # 목록에서 골라 지우기 (확인 후)
disk-clean clean caches dev:.terraform --dry-run
```

## 분류

| 분류 | 뜻 | `clean` |
|---|---|---|
| 안전 | 앱이 알아서 다시 만듦 — `~/Library/Caches`, 로그, npm·uv·bun 캐시, Xcode DerivedData | 지움 (목록에서 기본 선택) |
| 재생성 가능 | 다시 받거나 빌드해야 함 — 샌드박스 앱 캐시, Cargo·Gradle·Maven·Go·pnpm 캐시, 프로젝트의 `node_modules`·`target`·`.terraform`·`.venv` … | 지움 (직접 골라야 함) |
| 지운 앱이 남긴 데이터 | 앱을 지운 뒤 남은 컨테이너와 그 앱의 `/var/folders` 캐시 — 아래 세 조건이 모두 맞을 때만 | 지움 (직접 골라야 함, 지우기 직전 재확인) |
| 직접 판단 | 사용자 데이터일 수 있음 — 앱 컨테이너(게임 리소스 등), Application Support, 로컬 AI 모델, 다운로드 | **거부** — 앱 안에서 정리 |
| 시스템 | 로컬 스냅샷, 절전 이미지·스왑, `/var/folders` | 정보만 |

개발 산출물은 이름만으로 고르지 않는다. 옆에 프로젝트 파일이 있어야 한다
(`node_modules` ↔ `package.json`, `target` ↔ `Cargo.toml`/`pom.xml`/`CACHEDIR.TAG`, `.venv` ↔ `pyvenv.cfg` …).
`.terraform` 은 `.tf` 가 있거나, 코드가 지워진 뒤 남은 고아(`providers`·`modules` 가 든 것)도 잡는다.
모듈 clone 안에 중첩된 `.terraform` 은 따로 경고한다.

앱이 지워졌다는 판정은 셋이 모두 맞아야 한다: 컨테이너 메타데이터에 기록된 앱 경로가 없고,
LaunchServices 에 그 bundle id 로 등록된 앱이 없고, Spotlight 에서도 찾을 수 없을 것.
외장 디스크(`/Volumes/…`)에 있던 앱은 꽂혀 있지 않은 것과 구별이 안 되므로 판정하지 않는다.

## 안전장치

- `scan`·`top`·`--dry-run` 은 아무것도 바꾸지 않는다.
- 지우기 전에 항목·경로·크기를 보여 주고 `y` 를 받는다. 터미널이 아니면 `--yes` 없이는 지우지 않는다.
- 홈 폴더 밖, 홈 바로 아래, 보호 목록(`~/Library/Caches` 자체, `~/Documents` …), 상위 경로에 심볼릭 링크가 낀 곳은 지우지 않는다.
- 심볼릭 링크는 따라가지 않고, 다른 볼륨으로 넘어가지 않는다. `com.apple.*` 캐시는 건드리지 않는다.
- 개발 산출물은 지우기 직전에 표식을 다시 확인한다.
- 지운 뒤 늘어난 공간은 추정이 아니라 `statfs` 로 잰 값을 보여 준다.

## 측정

- 크기는 할당된 블록(`st_blocks`) 기준이라 sparse 파일이 부풀지 않는다. 하드 링크는 한 번만 센다.
  APFS 복제본(clone)은 구분하지 못해 실제보다 크게 잡힐 수 있다.
- 단위는 GiB(1024³). 설정 앱은 GB(1000³)라 7% 정도 크게 표시된다.
- macOS 는 다른 앱의 컨테이너를 읽는 시스템 호출을 가끔 중단시킨다(EINTR). 읽기·삭제는 이때 다시 시도하고,
  그래도 못 읽은 곳은 버리지 않고 "읽지 못한 곳" 으로 센다. `DISK_CLEAN_DEBUG=1` 로 경로와 오류를 볼 수 있다.
- 휴지통·메일·메시지·Safari 는 **전체 디스크 접근 권한**이 있어야 잰다. 권한이 없으면 그렇다고 표시하고,
  읽지 못한 폴더 수를 보여 준다. 권한은 이 프로그램이 아니라 실행하는 터미널 앱에 준다
  (시스템 설정 › 개인정보 보호 및 보안 › 전체 디스크 접근 권한).
