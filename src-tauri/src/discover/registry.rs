//! Claude 세션 등록부 (PLAN 4.4.1)

use std::fs;
use std::path::Path;

use serde::Deserialize;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// `<home>\sessions\<pid>.json` 한 개. 알 수 없는 필드는 무시한다.
/// 필수는 pid, sessionId 뿐이다. 선택 필드는 타입이 달라도 그 필드만 None 이 된다.
/// updatedAt 은 epoch ms 숫자다 (개발 PC 파일에서 확인됨)
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub pid: u32,
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub entrypoint: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub name: Option<String>,
    /// `idle`, `busy` 확인됨. 허가 대기 값은 미확인이라 문자열로 둔다
    #[serde(default, deserialize_with = "lenient")]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub updated_at: Option<i64>,
}

/// 타입이 맞지 않는 값은 에러 대신 None
fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).ok())
}

/// `<home>\sessions\*.json` 을 모두 읽는다. 읽거나 해석할 수 없는 파일은 건너뛴다.
/// 같은 폴더의 `.key` 파일은 용도를 확인하지 않았고 비밀일 수 있어 열지 않는다 (`.json` 만 연다)
pub fn read_registry(home: &Path) -> Vec<RegistryEntry> {
    let Ok(dir) = fs::read_dir(home.join("sessions")) else {
        return Vec::new();
    };
    dir.filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| serde_json::from_slice(&fs::read(p).ok()?).ok())
        .collect()
}

/// 등록부 중 pid가 살아 있는 것만
pub fn live_sessions(home: &Path) -> Vec<RegistryEntry> {
    let mut v = read_registry(home);
    v.retain(|e| is_alive(e.pid));
    v
}

// ponytail: pid 재사용(죽은 세션의 pid를 다른 프로세스가 받음)을 못 가린다, 등록부의 procStart와 프로세스 생성 시각을 대조할 때 올린다
pub fn is_alive(pid: u32) -> bool {
    // SAFETY: 핸들은 이 함수 안에서 열고 닫는다
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // 권한이 높은 프로세스는 열 수 없어도 살아 있다
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn home_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions");
        fs::create_dir(&dir).unwrap();
        for (name, body) in files {
            fs::write(dir.join(name), body).unwrap();
        }
        home
    }

    #[test]
    fn reads_good_files_and_skips_bad_ones() {
        let home = home_with(&[
            (
                "1.json",
                r#"{"pid":1,"sessionId":"s1","cwd":"C:\\x","name":"n","status":"busy","updatedAt":2000,"extra":{"a":1}}"#,
            ),
            ("2.json", r#"{"pid":2,"sessionId":"#), // 쓰는 중에 깨짐
            ("3.json", r#"{"pid":3}"#),             // 필수 필드 누락
            ("4.json", r#"{"pid":"4","sessionId":"s4"}"#), // 타입 다름
            ("5.json", r#"{"pid":5,"sessionId":"s5"}"#),
            // 선택 필드 타입이 달라도 파일은 살고 그 필드만 None
            (
                "6.json",
                r#"{"pid":6,"sessionId":"s6","updatedAt":"x","status":1,"name":"n6"}"#,
            ),
        ]);
        let mut got = read_registry(home.path());
        got.sort_by_key(|e| e.pid);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].session_id, "s1");
        assert_eq!(got[0].status.as_deref(), Some("busy"));
        assert_eq!(got[0].updated_at, Some(2000));
        assert_eq!(got[1].pid, 5);
        assert_eq!(got[1].name, None);
        assert_eq!(got[2].pid, 6);
        assert_eq!(got[2].updated_at, None);
        assert_eq!(got[2].status, None);
        assert_eq!(got[2].name.as_deref(), Some("n6"));
    }

    #[test]
    fn missing_sessions_dir_gives_empty_list() {
        let home = tempfile::tempdir().unwrap();
        assert!(read_registry(home.path()).is_empty());
    }

    #[test]
    fn key_files_are_not_read() {
        // .key 가 유효한 JSON이어도 결과에 들어오면 안 된다
        let home = home_with(&[("7.abcdef.key", r#"{"pid":7,"sessionId":"secret"}"#)]);
        assert!(read_registry(home.path()).is_empty());
    }

    #[test]
    fn own_pid_is_alive_and_exited_child_is_dead() {
        assert!(is_alive(std::process::id()));
        let mut child = Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!is_alive(pid));
    }

    #[test]
    fn live_sessions_drops_dead_pids() {
        let me = std::process::id();
        let home = home_with(&[
            ("a.json", &format!(r#"{{"pid":{me},"sessionId":"live"}}"#)),
            ("b.json", r#"{"pid":4294967294,"sessionId":"dead"}"#),
        ]);
        let got = live_sessions(home.path());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].session_id, "live");
    }
}
