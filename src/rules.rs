//! Where macOS and common tools keep data, and what happens if it goes away.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// Apps rebuild it on their own.
    Safe,
    /// Comes back, but costs a download or a rebuild.
    Regenerable,
    /// Belongs to an app that is no longer installed.
    Leftover,
    /// May be the user's own data; reported, never cleaned.
    Review,
}

impl Safety {
    pub fn title(self) -> &'static str {
        match self {
            Safety::Safe => "안전 — 지워도 앱이 알아서 다시 만듦",
            Safety::Regenerable => "재생성 가능 — 다시 받거나 빌드하는 시간이 듦",
            Safety::Leftover => "지운 앱이 남긴 데이터 — 앱은 이미 없음, 다시 설치하면 처음부터",
            Safety::Review => "직접 판단 — 사용자 데이터일 수 있어 지우지 않음",
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Safety::Safe => "안전",
            Safety::Regenerable => "재생성",
            Safety::Leftover => "잔여",
            Safety::Review => "판단",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Delete what is inside each target; keep the target directory itself.
    Contents,
    /// Delete each target as a whole.
    Whole,
    /// Report the children of each target.
    ReportChildren,
    /// Report each target as one entry.
    ReportSelf,
}

pub struct Rule {
    pub id: &'static str,
    pub name: &'static str,
    pub safety: Safety,
    pub mode: Mode,
    /// Relative to the home directory; a `*` component matches every directory there.
    pub targets: &'static [&'static str],
    pub exclude: Exclude,
    pub about: &'static str,
    /// Checked by default in the interactive picker.
    pub preselect: bool,
}

impl Rule {
    pub fn cleanable(&self) -> bool {
        matches!(self.mode, Mode::Contents | Mode::Whole)
    }
}

/// Entries left alone, and why.
pub struct Exclude {
    /// Entries whose names start with one of these.
    pub prefixes: &'static [&'static str],
    pub why: &'static str,
}

const NONE: Exclude = Exclude { prefixes: &[], why: "" };

/// Apple's own caches and daemons.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const APPLE: Exclude = Exclude { prefixes: &["com.apple."], why: "macOS 가 관리" };

use Mode::*;
use Safety::*;

/// Everywhere: tools that keep their data in the same place on every Unix.
const COMMON: &[Rule] = &[
    Rule {
        id: "npm",
        name: "npm 캐시",
        safety: Safe,
        mode: Whole,
        targets: &[".npm/_cacache"],
        exclude: NONE,
        about: "다음 npm install 때 다시 받음",
        preselect: true,
    },
    Rule {
        id: "uv",
        name: "uv 캐시",
        safety: Safe,
        mode: Whole,
        targets: &[".cache/uv"],
        exclude: NONE,
        about: "다음 uv sync 때 다시 받음",
        preselect: true,
    },
    Rule {
        id: "bun",
        name: "bun 캐시",
        safety: Safe,
        mode: Whole,
        targets: &[".bun/install/cache"],
        exclude: NONE,
        about: "다음 bun install 때 다시 받음",
        preselect: true,
    },
    Rule {
        id: "cargo",
        name: "Cargo 레지스트리",
        safety: Regenerable,
        mode: Whole,
        targets: &[".cargo/registry/cache", ".cargo/registry/src", ".cargo/git/checkouts"],
        exclude: NONE,
        about: "다음 cargo build 때 다시 받음",
        preselect: false,
    },
    Rule {
        id: "gradle",
        name: "Gradle 캐시",
        safety: Regenerable,
        mode: Whole,
        targets: &[".gradle/caches"],
        exclude: NONE,
        about: "다음 gradle 빌드 때 다시 받음",
        preselect: false,
    },
    Rule {
        id: "maven",
        name: "Maven 저장소",
        safety: Regenerable,
        mode: Whole,
        targets: &[".m2/repository"],
        exclude: NONE,
        about: "다음 mvn 빌드 때 다시 받음",
        preselect: false,
    },
    Rule {
        id: "go-mod",
        name: "Go 모듈 캐시",
        safety: Regenerable,
        mode: Whole,
        targets: &["go/pkg/mod"],
        exclude: NONE,
        about: "go clean -modcache 와 같음. 다음 빌드 때 다시 받음",
        preselect: false,
    },
    Rule {
        id: "terraform-plugins",
        name: "Terraform 플러그인 캐시",
        safety: Regenerable,
        mode: Whole,
        targets: &[".terraform.d/plugin-cache"],
        exclude: NONE,
        about: "다음 terraform init 때 다시 받음",
        preselect: false,
    },
    Rule {
        id: "models",
        name: "로컬 AI 모델",
        safety: Review,
        mode: ReportSelf,
        targets: &[".ollama", ".lmstudio", ".cache/huggingface", ".mtplx"],
        exclude: NONE,
        about: "다시 받을 수는 있지만 수십 GB 다운로드",
        preselect: false,
    },
    Rule {
        id: "downloads",
        name: "다운로드 폴더",
        safety: Review,
        mode: ReportChildren,
        targets: &["Downloads"],
        exclude: NONE,
        about: "직접 받은 파일",
        preselect: false,
    },
];

/// macOS: `~/Library` and Apple's developer tools.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MACOS: &[Rule] = &[
    Rule {
        id: "caches",
        name: "앱 캐시",
        safety: Safe,
        mode: Contents,
        targets: &["Library/Caches"],
        exclude: APPLE,
        about: "앱이 다시 받아 오거나 다시 만드는 데이터. 실행 중인 앱은 끄고 지우는 편이 깔끔함",
        preselect: true,
    },
    Rule {
        id: "logs",
        name: "로그·진단 보고서",
        safety: Safe,
        mode: Contents,
        targets: &["Library/Logs"],
        exclude: NONE,
        about: "앱 로그와 크래시 리포트",
        preselect: true,
    },
    Rule {
        id: "xcode-derived",
        name: "Xcode DerivedData",
        safety: Safe,
        mode: Contents,
        targets: &["Library/Developer/Xcode/DerivedData"],
        exclude: NONE,
        about: "Xcode 빌드 산출물. 다음 빌드 때 다시 만듦",
        preselect: true,
    },
    Rule {
        id: "mail-downloads",
        name: "메일 첨부 사본",
        safety: Safe,
        mode: Contents,
        targets: &["Library/Containers/com.apple.mail/Data/Library/Mail Downloads"],
        exclude: NONE,
        about: "메일에서 첨부파일을 열 때 만든 사본. 원본은 메일에 남음",
        preselect: true,
    },
    Rule {
        id: "trash",
        name: "휴지통",
        safety: Safe,
        mode: Contents,
        targets: &[".Trash"],
        exclude: NONE,
        about: "이미 버린 파일. 지우면 되돌릴 수 없음",
        preselect: false,
    },
    Rule {
        id: "container-caches",
        name: "샌드박스 앱 캐시",
        safety: Regenerable,
        mode: Contents,
        targets: &["Library/Containers/*/Data/Library/Caches"],
        exclude: APPLE,
        about: "App Store·샌드박스 앱의 캐시. 게임은 셰이더·리소스를 다시 받을 수 있음",
        preselect: false,
    },
    Rule {
        id: "xcode-device-support",
        name: "Xcode 기기 지원 파일",
        safety: Regenerable,
        mode: Contents,
        targets: &[
            "Library/Developer/Xcode/iOS DeviceSupport",
            "Library/Developer/Xcode/watchOS DeviceSupport",
            "Library/Developer/Xcode/tvOS DeviceSupport",
        ],
        exclude: NONE,
        about: "기기를 다시 연결하면 Xcode 가 다시 복사함",
        preselect: false,
    },
    Rule {
        id: "simulator-caches",
        name: "시뮬레이터 캐시",
        safety: Regenerable,
        mode: Whole,
        targets: &["Library/Developer/CoreSimulator/Caches"],
        exclude: NONE,
        about: "시뮬레이터를 띄울 때 다시 만듦",
        preselect: false,
    },
    Rule {
        id: "pnpm",
        name: "pnpm 저장소",
        safety: Regenerable,
        mode: Whole,
        targets: &["Library/pnpm/store"],
        exclude: NONE,
        about: "다음 pnpm install 때 다시 받음",
        preselect: false,
    },
    Rule {
        id: "containers",
        name: "앱 컨테이너 데이터",
        safety: Review,
        mode: ReportChildren,
        targets: &["Library/Containers"],
        exclude: NONE,
        about: "샌드박스 앱이 저장한 데이터(위 캐시 포함). 앱 안에서 정리하거나 앱을 지울 때 함께 삭제",
        preselect: false,
    },
    Rule {
        id: "group-containers",
        name: "앱 그룹 데이터",
        safety: Review,
        mode: ReportChildren,
        targets: &["Library/Group Containers"],
        exclude: NONE,
        about: "여러 앱이 함께 쓰는 데이터",
        preselect: false,
    },
    Rule {
        id: "app-support",
        name: "Application Support",
        safety: Review,
        mode: ReportChildren,
        targets: &["Library/Application Support"],
        exclude: NONE,
        about: "앱의 설정·데이터베이스·오프라인 데이터",
        preselect: false,
    },
    Rule {
        id: "xcode-archives",
        name: "Xcode Archives",
        safety: Review,
        mode: ReportSelf,
        targets: &["Library/Developer/Xcode/Archives"],
        exclude: NONE,
        about: "배포한 빌드와 dSYM. 크래시 분석에 필요할 수 있음",
        preselect: false,
    },
    Rule {
        id: "simulators",
        name: "iOS 시뮬레이터",
        safety: Review,
        mode: ReportSelf,
        targets: &["Library/Developer/CoreSimulator/Devices"],
        exclude: NONE,
        about: "`xcrun simctl delete unavailable` 로 안 쓰는 것만 지울 수 있음",
        preselect: false,
    },
    Rule {
        id: "messages",
        name: "메시지 첨부파일",
        safety: Review,
        mode: ReportSelf,
        targets: &["Library/Messages/Attachments"],
        exclude: NONE,
        about: "iMessage 로 주고받은 사진·파일",
        preselect: false,
    },
];

/// Linux: XDG folders and Flatpak.
#[cfg_attr(target_os = "macos", allow(dead_code))]
const LINUX: &[Rule] = &[
    Rule {
        id: "caches",
        name: "앱 캐시",
        safety: Safe,
        mode: Contents,
        targets: &[".cache"],
        exclude: Exclude {
            prefixes: &["huggingface", "uv"],
            why: "AI 모델과 uv 캐시는 따로 분류",
        },
        about: "XDG 캐시. 앱이 다시 받아 오거나 다시 만드는 데이터. 실행 중인 앱은 끄고 지우는 편이 깔끔함",
        preselect: true,
    },
    Rule {
        id: "trash",
        name: "휴지통",
        safety: Safe,
        mode: Contents,
        targets: &[".local/share/Trash/files", ".local/share/Trash/info"],
        exclude: NONE,
        about: "이미 버린 파일. 지우면 되돌릴 수 없음",
        preselect: false,
    },
    Rule {
        id: "flatpak-caches",
        name: "Flatpak 앱 캐시",
        safety: Regenerable,
        mode: Contents,
        targets: &[".var/app/*/cache"],
        exclude: NONE,
        about: "Flatpak 앱마다 따로 둔 캐시. 앱이 다시 만듦",
        preselect: false,
    },
    Rule {
        id: "pnpm",
        name: "pnpm 저장소",
        safety: Regenerable,
        mode: Whole,
        targets: &[".local/share/pnpm/store"],
        exclude: NONE,
        about: "다음 pnpm install 때 다시 받음",
        preselect: false,
    },
    Rule {
        id: "app-data",
        name: "앱 데이터",
        safety: Review,
        mode: ReportChildren,
        targets: &[".local/share"],
        exclude: Exclude { prefixes: &["Trash"], why: "휴지통은 따로 분류" },
        about: "앱의 데이터베이스·오프라인 데이터·게임(~/.local/share)",
        preselect: false,
    },
    Rule {
        id: "flatpak-apps",
        name: "Flatpak 앱 데이터",
        safety: Review,
        mode: ReportChildren,
        targets: &[".var/app"],
        exclude: NONE,
        about: "Flatpak 앱이 저장한 데이터(위 캐시 포함). 앱 안에서 정리하거나 앱을 지울 때 함께 삭제",
        preselect: false,
    },
];

#[cfg(target_os = "macos")]
const PLATFORM: &[Rule] = MACOS;
#[cfg(not(target_os = "macos"))]
const PLATFORM: &[Rule] = LINUX;

/// The rules for the system this was built for.
pub fn rules() -> impl Iterator<Item = &'static Rule> {
    COMMON.iter().chain(PLATFORM)
}

/// What a well-known file or folder name usually is.
pub fn hint(name: &str) -> Option<&'static str> {
    Some(match name {
        "node_modules" | "target" | ".terraform" | ".venv" | "venv" | "DerivedData"
        | "__pycache__" | ".next" | ".gradle" => "다시 만들 수 있는 개발 산출물일 수 있음",
        "Caches" => "캐시",
        "Containers" => "샌드박스 앱 데이터 — scan 의 '앱 컨테이너 데이터'",
        "Group Containers" => "앱 그룹 데이터",
        "Application Support" => "앱 데이터",
        ".Trash" => "휴지통",
        "MobileSync" => "iPhone·iPad 백업 — Finder › 기기 › 백업 관리",
        "com.docker.docker" => "Docker 디스크 이미지 — docker system prune 또는 Docker 설정 › Resources",
        "CoreSimulator" => "iOS 시뮬레이터",
        "sleepimage" => "절전 이미지",
        "docker" => "Docker 데이터(rootless) — docker system prune",
        "containers" => "Podman 이미지 — podman system prune",
        "flatpak" => "Flatpak 런타임 — flatpak uninstall --unused",
        "Steam" => "Steam 게임 — Steam 에서 정리",
        ".cache" => "캐시",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn each_platform() -> [(&'static str, Vec<&'static Rule>); 2] {
        [
            ("macos", COMMON.iter().chain(MACOS).collect()),
            ("linux", COMMON.iter().chain(LINUX).collect()),
        ]
    }

    #[test]
    fn ids_are_unique_on_each_platform() {
        for (os, rules) in each_platform() {
            let mut ids: Vec<_> = rules.iter().map(|r| r.id).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), rules.len(), "{os}");
        }
    }

    #[test]
    fn review_rules_are_never_cleanable() {
        for (_, rules) in each_platform() {
            for r in rules.iter().filter(|r| r.safety == Review) {
                assert!(!r.cleanable(), "{}", r.id);
            }
        }
    }

    #[test]
    fn targets_stay_below_home() {
        for (_, rules) in each_platform() {
            for t in rules.iter().flat_map(|r| r.targets) {
                assert!(!t.starts_with('/') && !t.split('/').any(|c| c == ".." || c.is_empty()), "{t}");
            }
        }
    }

    #[test]
    fn model_weights_never_count_as_cache() {
        let linux_caches = LINUX.iter().find(|r| r.id == "caches").unwrap();
        assert!(linux_caches.exclude.prefixes.contains(&"huggingface"));
        assert!(COMMON.iter().any(|r| r.id == "models" && r.safety == Review && r.targets.contains(&".cache/huggingface")));
    }
}
