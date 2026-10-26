//! 설정 파일 `settings.json` (docs/CONTRACTS.md C9 `get_settings`/`set_settings`).
//! 모르는 칸은 읽은 대로 두고 다시 쓴다.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};

use serde_json::{Map, Value};

use crate::ipc::{NightMode, SettingsDto};
use crate::office::names::{unwritable, write_atomic};
use crate::office::world::Scale;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub enum ManualVendor {
    Claude,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub struct ManualPath {
    pub vendor: ManualVendor,
    pub root: String,
}

impl ManualPath {
    /// WSL을 깨우지 않도록 존재 여부는 확인하지 않는다. 실제 접근은 Probe 뒤에 한다.
    pub fn validate(&self) -> Result<(), String> {
        let root = self.root.replace('/', "\\");
        let bytes = root.as_bytes();
        let rest = if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'\\'
        {
            &root[3..]
        } else if let Some(unc) = root.strip_prefix("\\\\") {
            let Some((server, rest)) = unc.split_once('\\') else {
                return Err("수동 경로의 WSL 배포판 이름이 없습니다".into());
            };
            if !(server.eq_ignore_ascii_case("wsl.localhost")
                || server.eq_ignore_ascii_case("wsl$"))
                || rest.is_empty()
                || rest.starts_with('\\')
            {
                return Err("수동 경로는 WSL 공유만 사용할 수 있습니다".into());
            }
            rest
        } else {
            return Err("수동 경로는 Windows 절대 경로나 WSL UNC 경로여야 합니다".into());
        };
        if rest.contains("\\\\")
            || rest
                .chars()
                .any(|c| c.is_control() || "<>:\"|?*".contains(c))
            || rest.trim_end_matches('\\').split('\\').any(|part| {
                part == "."
                    || part == ".."
                    || part.ends_with([' ', '.'])
                    || (part.is_empty() && !rest.is_empty())
            })
        {
            return Err("수동 경로에 잘못된 문자나 상위 경로가 있습니다".into());
        }
        Ok(())
    }
}

struct Inner {
    path: PathBuf,
    /// 읽지 못한 파일(잠김, 권한)은 덮어쓰지 않는다
    writable: bool,
    /// 파일 전체. 규모 말고도 모르는 칸을 그대로 들고 있다
    map: Map<String, Value>,
}

pub struct SettingsBook {
    inner: Mutex<Inner>,
    /// 규모가 정해지면 깨운다 (기동이 첫 선택을 기다린다)
    chosen: Condvar,
    /// 파일이 있는데 JSON 객체가 아니었다 (docs/CONTRACTS.md C3 "설정 파일 손상")
    pub corrupt: bool,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn scale_of(g: &Inner) -> Option<Scale> {
    serde_json::from_value(g.map.get("scale")?.clone()).ok()
}

/// 회사 이름·사장님 이름은 글자(char) 수로 센다.
const NAME_MAX: usize = 20;

/// 앞뒤 공백을 떼고 상한까지 자른다. 비면 None (기본 이름)
fn company_of(s: &str) -> Option<String> {
    let s: String = s.trim().chars().take(NAME_MAX).collect();
    (!s.is_empty()).then_some(s)
}

impl SettingsBook {
    /// 없거나 손상된 파일은 기본값(규모 없음)으로 시작한다. 손상이면 다음 저장이 덮어쓴다
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("settings.json");
        let (map, writable, corrupt) = match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice::<Value>(&b) {
                Ok(Value::Object(m)) => (m, true, false),
                _ => (Map::new(), true, true),
            },
            Err(e) => (Map::new(), e.kind() == io::ErrorKind::NotFound, false),
        };
        Self {
            inner: Mutex::new(Inner {
                path,
                writable,
                map,
            }),
            chosen: Condvar::new(),
            corrupt,
        }
    }

    /// 없거나 알 수 없는 값이면 None. 부르는 쪽이 규모 선택부터 시킨다
    pub fn scale(&self) -> Option<Scale> {
        scale_of(&lock(&self.inner))
    }

    /// 규모가 정해질 때까지 기다린다. 이미 있으면 바로 돌아온다.
    /// 기다리는 스레드는 detach 되어 있어 앱이 꺼질 때 프로세스 종료를 막지 않는다
    pub fn wait_scale(&self) -> Scale {
        let mut g = lock(&self.inner);
        loop {
            if let Some(s) = scale_of(&g) {
                return s;
            }
            g = self.chosen.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// 메모리에는 항상 반영한다. 저장에 실패하면 에러를 돌려주되 값은 남는다
    pub fn set_scale(&self, scale: Scale) -> io::Result<()> {
        self.update(&SettingsDto {
            scale: Some(scale),
            ..SettingsDto::default()
        })
    }

    /// 저장된 회사 이름. 없으면 None 이고 기본 이름은 프론트가 정한다. 손으로 고친 파일도 같은 규칙으로 읽는다
    pub fn company_name(&self) -> Option<String> {
        company_of(lock(&self.inner).map.get("company_name")?.as_str()?)
    }

    pub fn os_notifications(&self) -> bool {
        lock(&self.inner)
            .map
            .get("os_notifications")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    /// None 이나 공백뿐이면 키를 지운다(기본 이름). 메모리·저장 실패는 set_scale 과 같다
    pub fn set_company_name(&self, name: Option<&str>) -> io::Result<()> {
        self.update(&SettingsDto {
            company_name: Some(name.unwrap_or_default().into()),
            ..SettingsDto::default()
        })
    }

    /// 잘못된 입력은 아무 칸도 바꾸지 않는다. 저장 실패는 모든 칸을 이번 실행에 유지한다.
    pub fn update(&self, dto: &SettingsDto) -> io::Result<()> {
        if let Some(paths) = &dto.manual_paths {
            for path in paths {
                path.validate()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            }
        }
        if dto == &SettingsDto::default() {
            return Ok(());
        }
        let mut g = lock(&self.inner);
        if let Some(scale) = dto.scale {
            g.map.insert("scale".into(), serde_json::to_value(scale)?);
            self.chosen.notify_all();
        }
        for (key, name) in [
            ("company_name", &dto.company_name),
            ("boss_name", &dto.boss_name),
        ] {
            if let Some(name) = name {
                match company_of(name) {
                    Some(name) => g.map.insert(key.into(), name.into()),
                    None => g.map.remove(key),
                };
            }
        }
        for (key, value) in [
            ("sound", dto.sound),
            ("os_notifications", dto.os_notifications),
            ("raw_diagnostics", dto.raw_diagnostics),
        ] {
            if let Some(value) = value {
                g.map.insert(key.into(), value.into());
            }
        }
        if let Some(paths) = &dto.manual_paths {
            g.map
                .insert("manual_paths".into(), serde_json::to_value(paths)?);
        }
        if let Some(mode) = dto.night_mode {
            g.map
                .insert("night_mode".into(), serde_json::to_value(mode)?);
            g.map.remove("nightMode");
        }
        if let Some(animal) = dto.animal {
            g.map.insert("animal".into(), serde_json::to_value(animal)?);
        }
        if !g.writable {
            return Err(unwritable(&g.path));
        }
        let bytes = serde_json::to_vec_pretty(&g.map)?;
        write_atomic(&g.path, &bytes)
    }

    /// 파일을 손으로 고친 경우에도 IPC 입력과 같은 경로 검증·이름 상한을 적용한다.
    pub fn snapshot(&self) -> SettingsDto {
        let g = lock(&self.inner);
        let name = |key| g.map.get(key).and_then(Value::as_str).and_then(company_of);
        let flag = |key, default| g.map.get(key).and_then(Value::as_bool).unwrap_or(default);
        let paths = g
            .map
            .get("manual_paths")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| serde_json::from_value::<ManualPath>(v.clone()).ok())
            .filter(|path| path.validate().is_ok())
            .collect();
        SettingsDto {
            scale: scale_of(&g),
            company_name: name("company_name"),
            manual_paths: Some(paths),
            sound: Some(flag("sound", false)),
            night_mode: Some(
                g.map
                    .get("night_mode")
                    .or_else(|| g.map.get("nightMode"))
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or(NightMode::Auto),
            ),
            os_notifications: Some(flag("os_notifications", true)),
            boss_name: name("boss_name"),
            raw_diagnostics: Some(flag("raw_diagnostics", false)),
            animal: g
                .map
                .get("animal")
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Animal;

    fn read(dir: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap()
    }

    #[test]
    fn night_mode_defaults_for_missing_old_or_invalid_settings() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            SettingsBook::load(dir.path()).snapshot().night_mode,
            Some(NightMode::Auto)
        );
        for value in [
            serde_json::json!({"scale": "Small", "sound": true}),
            serde_json::json!({"nightMode": "unknown"}),
            serde_json::json!({"nightMode": true}),
            serde_json::json!({"nightMode": null}),
            serde_json::json!({"night_mode": "unknown", "nightMode": "on"}),
            serde_json::json!({"night_mode": true, "nightMode": "on"}),
            serde_json::json!({"night_mode": null, "nightMode": "on"}),
        ] {
            std::fs::write(dir.path().join("settings.json"), value.to_string()).unwrap();
            assert_eq!(
                SettingsBook::load(dir.path()).snapshot().night_mode,
                Some(NightMode::Auto)
            );
        }
    }

    #[test]
    fn night_mode_legacy_fallback_canonical_priority_and_patch_preservation() {
        let dir = tempfile::tempdir().unwrap();
        for (stored, expected) in [
            (
                serde_json::json!({"nightMode": "on", "future": [1, 2]}),
                NightMode::On,
            ),
            (
                serde_json::json!({"night_mode": "off", "nightMode": "on", "future": [1, 2]}),
                NightMode::Off,
            ),
        ] {
            std::fs::write(dir.path().join("settings.json"), stored.to_string()).unwrap();
            let book = SettingsBook::load(dir.path());
            assert_eq!(book.snapshot().night_mode, Some(expected));
            let patch = serde_json::from_value(serde_json::json!({
                "nightMode": null,
                "sound": true
            }))
            .unwrap();
            book.update(&patch).unwrap();
            let mut after_partial = stored.clone();
            after_partial["sound"] = true.into();
            assert_eq!(read(dir.path()), after_partial);
            assert_eq!(
                SettingsBook::load(dir.path()).snapshot().night_mode,
                Some(expected)
            );

            let patch = serde_json::from_value(serde_json::json!({"nightMode": "auto"})).unwrap();
            book.update(&patch).unwrap();
            after_partial.as_object_mut().unwrap().remove("nightMode");
            after_partial["night_mode"] = "auto".into();
            assert_eq!(read(dir.path()), after_partial);
            assert_eq!(
                SettingsBook::load(dir.path()).snapshot().night_mode,
                Some(NightMode::Auto)
            );
        }
    }

    #[test]
    fn animal_defaults_to_none_ignores_unknown_and_survives_partial_updates() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(SettingsBook::load(dir.path()).snapshot().animal, None);
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"animal": "fish", "future": [1]}"#,
        )
        .unwrap();
        let book = SettingsBook::load(dir.path());
        assert_eq!(book.snapshot().animal, None);
        let patch = |v| serde_json::from_value::<SettingsDto>(v).unwrap();
        book.update(&patch(serde_json::json!({"animal": "dogs"})))
            .unwrap();
        book.update(&patch(serde_json::json!({"animal": null, "sound": true})))
            .unwrap();
        assert_eq!(read(dir.path())["animal"], "dogs");
        assert_eq!(read(dir.path())["future"], serde_json::json!([1]));
        assert_eq!(
            SettingsBook::load(dir.path()).snapshot().animal,
            Some(Animal::Dogs)
        );
    }

    #[test]
    fn night_mode_round_trip_partial_null_and_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let book = SettingsBook::load(dir.path());
        for mode in ["on", "off", "auto"] {
            let patch: SettingsDto =
                serde_json::from_value(serde_json::json!({"nightMode": mode})).unwrap();
            book.update(&patch).unwrap();
            assert_eq!(read(dir.path())["night_mode"], mode);
            assert!(read(dir.path()).get("nightMode").is_none());
            assert_eq!(
                SettingsBook::load(dir.path()).snapshot().night_mode,
                patch.night_mode
            );
            let null_patch =
                serde_json::from_value(serde_json::json!({"nightMode": null, "sound": true}))
                    .unwrap();
            book.update(&null_patch).unwrap();
            assert_eq!(book.snapshot().night_mode, patch.night_mode);
            assert_eq!(book.snapshot().sound, Some(true));
        }
        for invalid in [
            serde_json::json!("Auto"),
            serde_json::json!("unknown"),
            serde_json::json!(true),
        ] {
            assert!(
                serde_json::from_value::<SettingsDto>(serde_json::json!({"nightMode": invalid}))
                    .is_err()
            );
        }
    }

    #[test]
    fn saves_and_restores_scale() {
        let dir = tempfile::tempdir().unwrap();
        let b = SettingsBook::load(dir.path());
        assert_eq!(b.scale(), None);
        b.set_scale(Scale::Small).unwrap();
        assert_eq!(SettingsBook::load(dir.path()).scale(), Some(Scale::Small));
    }

    #[test]
    fn keeps_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"scale":"Large","sound":{"on":true},"future":[1,2]}"#,
        )
        .unwrap();
        let b = SettingsBook::load(dir.path());
        assert_eq!(b.scale(), Some(Scale::Large));
        b.set_scale(Scale::Small).unwrap();
        let v = read(dir.path());
        assert_eq!(v["scale"], "Small");
        assert_eq!(v["sound"]["on"], true);
        assert_eq!(v["future"][1], 2);
    }

    /// 이름은 다듬어 저장되고 다시 열어도 같다. 다른 칸은 그대로 남는다
    #[test]
    fn saves_and_restores_company_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"scale":"Small","future":1}"#,
        )
        .unwrap();
        let b = SettingsBook::load(dir.path());
        assert_eq!(b.company_name(), None);
        b.set_company_name(Some("  야근상사 ")).unwrap();
        let again = SettingsBook::load(dir.path());
        assert_eq!(again.company_name().as_deref(), Some("야근상사"));
        assert_eq!(again.scale(), Some(Scale::Small));
        assert_eq!(read(dir.path())["future"], 1);
    }

    /// 빈 문자열·공백뿐·None 은 키를 지워 기본 이름으로 돌아간다. 다른 칸은 그대로
    #[test]
    fn empty_company_name_removes_key() {
        for empty in [Some(""), Some("   "), None] {
            let dir = tempfile::tempdir().unwrap();
            let b = SettingsBook::load(dir.path());
            b.set_scale(Scale::Large).unwrap();
            b.set_company_name(Some("야근상사")).unwrap();
            b.set_company_name(empty).unwrap();
            assert_eq!(b.company_name(), None, "{empty:?}");
            let v = read(dir.path());
            assert!(v.get("company_name").is_none(), "{empty:?}");
            assert_eq!(v["scale"], "Large", "{empty:?}");
        }
    }

    /// 20자 넘는 이름은 글자 수로 20자에서 잘라 저장한다
    #[test]
    fn long_company_name_is_cut_to_20_chars() {
        let dir = tempfile::tempdir().unwrap();
        let b = SettingsBook::load(dir.path());
        b.set_company_name(Some(&"가".repeat(21))).unwrap();
        assert_eq!(read(dir.path())["company_name"], "가".repeat(20));
        assert_eq!(
            SettingsBook::load(dir.path()).company_name(),
            Some("가".repeat(20))
        );
    }

    #[test]
    fn corrupt_file_starts_from_defaults_and_is_overwritten() {
        for text in ["{ 깨짐", "[1]", "\"Large\"", r#"{"scale":"Huge"}"#] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("settings.json"), text).unwrap();
            let b = SettingsBook::load(dir.path());
            assert_eq!(b.scale(), None, "{text}");
            b.set_scale(Scale::Large).unwrap();
            assert_eq!(read(dir.path())["scale"], "Large", "{text}");
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), "{ 깨짐").unwrap();
        assert!(SettingsBook::load(dir.path()).corrupt);
    }

    /// 저장 폴더를 만들 수 없으면 에러를 돌려주되 이번 실행의 값은 메모리에 남는다
    #[test]
    fn save_failure_keeps_memory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "x").unwrap();
        let b = SettingsBook::load(&file);
        assert!(b.set_scale(Scale::Small).is_err());
        assert_eq!(b.scale(), Some(Scale::Small));
    }

    /// 읽지 못한 파일(여기서는 디렉터리)은 덮어쓰지 않는다
    #[test]
    fn unreadable_file_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("settings.json")).unwrap();
        let b = SettingsBook::load(dir.path());
        assert!(b.set_scale(Scale::Small).is_err());
        assert!(dir.path().join("settings.json").is_dir());
    }

    #[test]
    fn manual_paths_accept_only_absolute_local_or_wsl_roots() {
        let path = |root: &str| ManualPath {
            vendor: ManualVendor::Claude,
            root: root.into(),
        };
        for root in [
            r"C:\",
            r"D:\sessions\projects",
            "D:/sessions/projects",
            r"\\wsl.localhost\Ubuntu\home\fixture\.claude\projects",
            r"\\wsl$\Debian\home\fixture\.codex\sessions",
        ] {
            assert!(path(root).validate().is_ok(), "{root}");
        }
        for root in [
            "",
            "projects",
            r"C:projects",
            r"\projects",
            r"\\server\share\projects",
            r"\\wsl.localhost",
            r"\\wsl$\",
            r"\\wsl$\\projects",
            r"\\?\C:\projects",
            r"C:\projects\..\sessions",
            r"C:\projects\.. \sessions",
            r"\\wsl$\Ubuntu\home\..\sessions",
            r"C:\projects:stream",
            r"C:\projects\\sessions",
            "C:\\bad\nname",
            r"C:\projects\*",
            r"C:\projects\.",
        ] {
            assert!(path(root).validate().is_err(), "{root}");
        }
    }

    /// 파일에 잘못된 경로나 타입을 써도 감시 후보·원문 설정에 그대로 들어가지 않는다.
    #[test]
    fn hand_edited_settings_use_defaults_and_validate_paths() {
        let dir = tempfile::tempdir().unwrap();
        let value = serde_json::json!({
            "sound": {"on": true}, "os_notifications": "false", "raw_diagnostics": 1,
            "boss_name": format!("  {}  ", "가".repeat(21)),
            "manual_paths": [
                {"vendor": "Claude", "root": "D:/fixture/projects"},
                {"vendor": "Codex", "root": "../sessions"},
                {"vendor": "Gemini", "root": "D:/fixture/brain"}
            ], "future": {"value": 1}
        });
        std::fs::write(dir.path().join("settings.json"), value.to_string()).unwrap();
        let book = SettingsBook::load(dir.path());
        let snapshot = book.snapshot();
        assert_eq!(snapshot.sound, Some(false));
        assert_eq!(snapshot.os_notifications, Some(true));
        assert_eq!(snapshot.raw_diagnostics, Some(false));
        assert_eq!(snapshot.boss_name, Some("가".repeat(20)));
        assert_eq!(
            snapshot.manual_paths.unwrap(),
            vec![ManualPath {
                vendor: ManualVendor::Claude,
                root: "D:/fixture/projects".into()
            }]
        );
        book.set_scale(Scale::Small).unwrap();
        assert_eq!(read(dir.path())["future"], value["future"]);
    }
}
