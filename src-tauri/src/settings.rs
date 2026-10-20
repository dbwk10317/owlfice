//! 설정 파일 `settings.json` (PLAN 4.8 `get_settings`/`set_settings`).
//! 지금은 규모만 안다. 모르는 칸(T30 이후의 설정, 새 버전이 쓴 칸)은 읽은 대로 두고 다시 쓴다.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde_json::{Map, Value};

use crate::office::names::{unwritable, write_atomic};
use crate::office::world::Scale;

struct Inner {
    path: PathBuf,
    /// 읽지 못한 파일(잠김, 권한)은 덮어쓰지 않는다
    writable: bool,
    /// 파일 전체. 규모 말고도 모르는 칸을 그대로 들고 있다
    map: Map<String, Value>,
}

pub struct SettingsBook {
    inner: Mutex<Inner>,
    /// 파일이 있는데 JSON 객체가 아니었다 (3.9 "설정 파일 손상")
    pub corrupt: bool,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
            corrupt,
        }
    }

    /// 없거나 알 수 없는 값이면 None. 부르는 쪽이 규모 선택부터 시킨다
    pub fn scale(&self) -> Option<Scale> {
        let v = lock(&self.inner).map.get("scale")?.clone();
        serde_json::from_value(v).ok()
    }

    /// 메모리에는 항상 반영한다. 저장에 실패하면 에러를 돌려주되 값은 남는다
    pub fn set_scale(&self, scale: Scale) -> io::Result<()> {
        let mut g = lock(&self.inner);
        g.map.insert("scale".into(), serde_json::to_value(scale)?);
        if !g.writable {
            return Err(unwritable(&g.path));
        }
        let bytes = serde_json::to_vec_pretty(&g.map)?;
        write_atomic(&g.path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(dir: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap()
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
}
