//! 직원 이름, 방 번호, 층 번호 (PLAN 4.7.7, 4.7.8, 4.7.9, 4.4.2).
//! 저장 폴더는 인자로 받는다. 실제 경로 `%APPDATA%\Owlfice` 는 부르는 쪽이 정한다.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use crate::discover::{Host, HostKind};

const SURNAMES: [&str; 40] = [
    "김", "이", "박", "최", "정", "강", "조", "윤", "장", "임", "한", "오", "서", "신", "권", "황",
    "안", "송", "류", "홍", "전", "고", "문", "양", "손", "배", "백", "허", "유", "남", "심", "노",
    "하", "곽", "성", "차", "주", "우", "구", "민",
];

const GIVEN_NAMES: &[&str] = &[
    "하늘", "서준", "지우", "도윤", "예린", "민재", "수아", "서윤", "지호", "하준", "시우", "은우",
    "지안", "하윤", "예준", "주원", "지유", "유준", "도현", "건우", "현우", "지훈", "우진", "선우",
    "서진", "연우", "민준", "유찬", "윤우", "은찬", "이준", "지원", "승우", "승현", "시윤", "시현",
    "지후", "준서", "준우", "지환", "현준", "도하", "도영", "도훈", "도진", "도경", "도원", "도연",
    "동현", "동욱", "동하", "동민", "동진", "동준", "동원", "민석", "민호", "민규", "민성", "민수",
    "민우", "민혁", "민기", "민영", "민아", "민지", "민서", "민주", "민정", "민경", "민선", "서연",
    "서현", "서영", "서아", "서하", "서은", "서희", "서정", "서율", "서원", "수빈", "수현", "수진",
    "수연", "수민", "수정", "수지", "수영", "수희", "수호", "수혁", "가온", "가은", "가현", "가윤",
    "가인", "나연", "나은", "나윤", "나래", "나리", "다은", "다인", "다온", "다현", "다빈", "다연",
    "다영", "다희", "다솜", "라온", "라희", "라윤", "라엘", "채원", "채윤", "채은", "채린", "채영",
    "채아", "채현", "채이", "예은", "예나", "예서", "예지", "예진", "예원", "예솔", "예담", "유나",
    "유진", "유리", "유빈", "유정", "유라", "유민", "유하", "유성", "유겸", "윤서", "윤아", "윤지",
    "윤호", "윤성", "윤재", "윤슬", "은서", "은지", "은빈", "은진", "은호", "은성", "은솔", "은채",
    "지민", "지수", "지아", "지현", "지은", "지연", "지영", "지석", "지성", "지율", "지한", "지효",
    "하린", "하람", "하영", "하은", "하연", "하진", "하랑", "하율", "하민", "한결", "한별", "한솔",
    "한빛", "한울", "해인", "해솔", "해원", "해린", "해성", "해찬", "현서", "현진", "현아", "현수",
    "현석", "현성", "현빈", "현정", "혜원", "혜진", "혜인", "혜림", "혜성", "효진", "효린", "효준",
    "효원", "태윤", "태양", "태민", "태준", "태현", "태성", "태희", "태이", "재현", "재민", "재윤",
    "재원", "재호", "재성", "재이", "재희", "정민", "정우", "정훈", "정현", "정원", "정호", "정수",
    "정아", "진우", "진서", "진아", "진호", "진영", "진성", "진혁", "진솔", "성민", "성준", "성현",
    "성우", "성진", "성호", "성윤", "소율", "소윤", "소연", "소희", "소민", "소은", "소영", "소담",
    "승민", "승준", "승호", "승윤", "승아", "승원", "시온", "시아", "시은", "시안", "시후", "아윤",
    "아인", "아린", "아라", "아름", "아현", "영진", "영훈", "영민", "영호", "영준", "영우", "우빈",
    "우성", "우혁", "우주", "원준", "원우", "원빈", "원재", "원호", "이안", "이든", "이서", "이솔",
    "이레", "이랑", "이현", "이수", "이슬", "이한", "인서", "인우", "인성", "인호", "준혁", "준호",
    "준영", "준희", "준수", "준기", "창민", "창현", "창호", "창준", "청아", "초연", "초은", "초원",
    "태오", "태리", "하겸", "해든", "해나", "호준", "호진", "호연", "호원", "호영", "규민", "규리",
    "규빈", "규현", "규진", "규원", "기쁨", "기현", "기태", "나현", "나영", "나경", "남준", "단아",
    "단비", "대현", "대윤", "대한", "대성", "대호", "도아", "도은", "도율", "로운", "루아", "루희",
    "마루", "미소", "미르", "미나", "미래", "보라", "보민", "보람", "보연", "보현", "사랑", "상현",
    "상우", "새롬", "새봄", "새별", "새연", "석진", "석현", "선아", "선율", "선재", "설아", "세린",
    "세아", "세은", "세희", "세준", "세현", "세진", "소라", "솔아", "솔비", "솔이", "송이", "슬기",
    "슬아", "시연", "신우", "아솔", "안나", "애린", "연수", "연서", "연아", "연지", "연재", "연호",
    "영서", "영은", "예하", "온유", "완호", "용준", "용진", "우림", "우솔", "은결", "은율", "은하",
    "이룸", "이삭",
];

/// 방 번호를 회수하는 기준. 팀이 마지막으로 보인 지 이만큼 지나면 번호를 비운 것으로 본다.
const ROOM_TTL_DAYS: i64 = 7;
const ROOMS_PER_FLOOR: u32 = 99;

/// FNV-1a 64. std 의 DefaultHasher 는 Rust 버전마다 값이 바뀔 수 있어 저장된 이름·번호와 어긋난다.
pub fn fnv1a64(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// 임시 파일에 쓴 뒤 이름을 바꾼다. 쓰다 끊겨도 기존 파일이 깨지지 않는다.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// 파일이 없거나 JSON 이 깨졌으면 빈 상태로 시작한다. 그 밖의 읽기 오류(잠김, 권한)면
/// 빈 상태로 계속 돌되 `false` 를 같이 돌려준다. 부르는 쪽은 그 파일에 쓰지 않는다.
fn load_json<T: Default + for<'a> Deserialize<'a>>(path: &Path) -> (T, bool) {
    match std::fs::read(path) {
        Ok(b) => (serde_json::from_slice(&b).unwrap_or_default(), true),
        Err(e) => (T::default(), e.kind() == io::ErrorKind::NotFound),
    }
}

fn unwritable(path: &Path) -> io::Error {
    io::Error::other(format!("{} 을 읽지 못해 쓰지 않음", path.display()))
}

fn save_json<T: Serialize>(path: &Path, v: &T) -> io::Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(v)?)
}

#[derive(Serialize, Deserialize, Clone)]
struct NameEntry {
    name: String,
    first_seen: String,
}

/// names.json: `{ "<agent_id>": {"name", "first_seen"} }`
pub struct NameBook {
    path: PathBuf,
    writable: bool,
    entries: BTreeMap<String, NameEntry>,
    /// 메모리에만 배정하고 아직 저장하지 않은 이름이 있다
    dirty: bool,
}

/// 잠금 밖에서 쓸 names.json 내용. world 잠금 안에서 파일을 쓰지 않으려고 꺼내 둔다
pub struct NamesSave {
    path: PathBuf,
    writable: bool,
    entries: BTreeMap<String, NameEntry>,
}

impl NamesSave {
    pub fn write(self) -> io::Result<()> {
        if self.writable {
            save_json(&self.path, &self.entries)
        } else {
            Err(unwritable(&self.path))
        }
    }
}

impl NameBook {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("names.json");
        let (entries, writable) = load_json(&path);
        Self {
            path,
            writable,
            entries,
            dirty: false,
        }
    }

    /// agent_id 의 이름. 저장된 이름이 있으면 그대로(같은 사무실에서 겹쳐도 유지),
    /// 없으면 시드 `hash(id)` 부터 `in_use` 에 없는 이름을 시드+1 씩 올려 뽑는다.
    /// 새로 뽑았을 때만 저장하며, 저장에 실패해도 이름은 메모리에 남는다.
    pub fn assign(
        &mut self,
        id: &str,
        in_use: &HashSet<String>,
        now: DateTime<Utc>,
    ) -> (String, io::Result<()>) {
        let name = self.assign_unsaved(id, in_use, now);
        let saved = self.unsaved().map_or(Ok(()), NamesSave::write);
        (name, saved)
    }

    /// `assign` 과 같지만 메모리에만 둔다. 저장은 `unsaved` 로 꺼내 부르는 쪽이 한다
    pub fn assign_unsaved(
        &mut self,
        id: &str,
        in_use: &HashSet<String>,
        now: DateTime<Utc>,
    ) -> String {
        if let Some(e) = self.entries.get(id) {
            return e.name.clone();
        }
        let seed = fnv1a64(id);
        let combos = (SURNAMES.len() * GIVEN_NAMES.len()) as u64;
        let mut name = String::new();
        // ponytail: 조합을 다 쓰면 마지막 후보가 중복으로 나간다, 한 사무실이 조합 수(약 만 명)에 닿을 때 올린다.
        for k in 0..combos {
            name = pick(seed.wrapping_add(k));
            if !in_use.contains(&name) {
                break;
            }
        }
        self.entries.insert(
            id.to_string(),
            NameEntry {
                name: name.clone(),
                first_seen: now.to_rfc3339(),
            },
        );
        self.dirty = true;
        name
    }

    /// 아직 저장하지 않은 이름이 있으면 쓸 내용을 꺼낸다
    pub fn unsaved(&mut self) -> Option<NamesSave> {
        std::mem::take(&mut self.dirty).then(|| NamesSave {
            path: self.path.clone(),
            writable: self.writable,
            entries: self.entries.clone(),
        })
    }
}

fn pick(h: u64) -> String {
    let n = SURNAMES.len() as u64;
    let surname = SURNAMES[(h % n) as usize];
    let given = GIVEN_NAMES[((h / n) % GIVEN_NAMES.len() as u64) as usize];
    format!("{surname}{given}")
}

#[derive(Serialize, Deserialize, Clone)]
struct RoomEntry {
    number: u32,
    last_seen: String,
}

#[derive(Serialize, Deserialize, Default)]
struct RoomsFile {
    /// 호스트 id -> 층. Windows 는 항상 1이라 넣지 않는다.
    floors: BTreeMap<String, u32>,
    /// 팀(session_id) -> 방
    rooms: BTreeMap<String, RoomEntry>,
}

pub struct RoomAssignment {
    pub number: u32,
    /// 한 층 99팀을 넘어 번호를 재사용했다. 부르는 쪽이 로그를 남긴다.
    pub reused: bool,
    pub saved: io::Result<()>,
}

/// rooms.json: 층 번호와 방 번호
pub struct RoomBook {
    path: PathBuf,
    writable: bool,
    /// 파일에 마지막으로 쓴(또는 읽은) 방별 last_seen. 메모리 값이 이보다 하루 넘게 앞서면 다시 쓴다.
    file_seen: BTreeMap<String, String>,
    data: RoomsFile,
}

impl RoomBook {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("rooms.json");
        let (data, writable): (RoomsFile, bool) = load_json(&path);
        let mut book = Self {
            path,
            writable,
            file_seen: BTreeMap::new(),
            data,
        };
        book.mark_file_seen();
        book
    }

    fn mark_file_seen(&mut self) {
        self.file_seen = self
            .data
            .rooms
            .iter()
            .map(|(k, r)| (k.clone(), r.last_seen.clone()))
            .collect();
    }

    pub fn save(&self) -> io::Result<()> {
        if !self.writable {
            return Err(unwritable(&self.path));
        }
        save_json(&self.path, &self.data)
    }

    fn persist(&mut self) -> io::Result<()> {
        self.save()?;
        self.mark_file_seen();
        Ok(())
    }

    /// 호스트의 층. Windows 는 1, 그 밖은 처음 본 순서로 2, 3 ... (이름순이면 새 배포판이 끼어들 때 어긋난다).
    /// 새로 매긴 층은 메모리에 두고 저장 결과를 같이 돌려준다.
    pub fn floor_of(&mut self, host: &Host) -> (u32, io::Result<()>) {
        if host.kind == HostKind::Windows {
            return (1, Ok(()));
        }
        if let Some(&f) = self.data.floors.get(&host.id) {
            return (f, Ok(()));
        }
        let floor = self.data.floors.values().copied().max().unwrap_or(1) + 1;
        self.data.floors.insert(host.id.clone(), floor);
        (floor, self.persist())
    }

    /// 팀의 방 번호(`floor*100+nn`). 저장된 번호는 그대로 쓰고 `last_seen` 만 갱신한다.
    /// 팀이 보일 때마다 부른다. 회수는 마지막 호출 뒤 7일이다.
    /// 파일에는 새 번호를 줄 때와, 파일의 last_seen 이 하루 넘게 낡았을 때 쓴다.
    /// 7일 넘게 안 보인 팀의 번호는 비운 것으로 본다. 그 팀이 돌아오면 새로 받는다.
    pub fn room_of(&mut self, session_id: &str, host: &Host, now: DateTime<Utc>) -> RoomAssignment {
        let (floor, floor_saved) = self.floor_of(host);
        let ttl = Duration::days(ROOM_TTL_DAYS);
        // 파싱 못 하는 시각은 오래된 것으로 본다.
        self.data.rooms.retain(|_, r| {
            DateTime::parse_from_rfc3339(&r.last_seen)
                .is_ok_and(|t| now.signed_duration_since(t) <= ttl)
        });

        if let Some(r) = self.data.rooms.get_mut(session_id) {
            r.last_seen = now.to_rfc3339();
            let number = r.number;
            let stale = self.file_seen.get(session_id).is_none_or(|s| {
                DateTime::parse_from_rfc3339(s)
                    .is_ok_and(|t| now.signed_duration_since(t) > Duration::days(1))
            });
            let saved = if stale { self.persist() } else { floor_saved };
            return RoomAssignment {
                number,
                reused: false,
                saved,
            };
        }

        let taken: HashSet<u32> = self
            .data
            .rooms
            .values()
            .filter(|r| r.number / 100 == floor)
            .map(|r| r.number % 100)
            .collect();
        let start = (fnv1a64(session_id) % u64::from(ROOMS_PER_FLOOR)) as u32;
        let free = (0..ROOMS_PER_FLOOR)
            .map(|i| (start + i) % ROOMS_PER_FLOOR + 1)
            .find(|nn| !taken.contains(nn));
        let nn = free.unwrap_or(start + 1);
        let number = floor * 100 + nn;
        self.data.rooms.insert(
            session_id.to_string(),
            RoomEntry {
                number,
                last_seen: now.to_rfc3339(),
            },
        );
        RoomAssignment {
            number,
            reused: free.is_none(),
            saved: self.persist(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn win() -> Host {
        Host {
            id: "windows".into(),
            kind: HostKind::Windows,
            label: "Windows".into(),
        }
    }

    fn wsl(distro: &str) -> Host {
        Host {
            id: format!("wsl:{distro}"),
            kind: HostKind::Wsl {
                distro: distro.into(),
            },
            label: distro.into(),
        }
    }

    fn t(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, day, 9, 0, 0).unwrap()
    }

    /// `base` 와 시작 번호가 같은 다른 session_id
    fn same_start(base: &str) -> String {
        let s = fnv1a64(base) % 99;
        (0..)
            .map(|i| format!("other{i}"))
            .find(|k| k != base && fnv1a64(k) % 99 == s)
            .unwrap()
    }

    #[test]
    fn pools_are_big_distinct_hangul() {
        let hangul = |s: &str| s.chars().all(|c| ('가'..='힣').contains(&c));
        assert!(GIVEN_NAMES.len() >= 300);
        assert!(
            GIVEN_NAMES
                .iter()
                .all(|n| n.chars().count() == 2 && hangul(n))
        );
        assert!(SURNAMES.iter().all(|n| n.chars().count() == 1 && hangul(n)));
        let given: HashSet<_> = GIVEN_NAMES.iter().collect();
        let sur: HashSet<_> = SURNAMES.iter().collect();
        assert_eq!(given.len(), GIVEN_NAMES.len());
        assert_eq!(sur.len(), SURNAMES.len());
    }

    #[test]
    fn hash_is_pinned() {
        // 저장된 이름·번호가 Rust 버전과 무관하게 같아야 한다.
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn name_is_deterministic_across_books() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let none = HashSet::new();
        let n1 = NameBook::load(a.path()).assign("agent-1", &none, t(1)).0;
        let n2 = NameBook::load(b.path()).assign("agent-1", &none, t(5)).0;
        assert_eq!(n1, n2);
        assert_eq!(n1.chars().count(), 3);
    }

    #[test]
    fn name_collision_takes_next_seed() {
        let dir = tempfile::tempdir().unwrap();
        let first = NameBook::load(dir.path())
            .assign("x", &HashSet::new(), t(1))
            .0;
        let other = tempfile::tempdir().unwrap();
        let used = HashSet::from([first.clone()]);
        let second = NameBook::load(other.path()).assign("x", &used, t(1)).0;
        assert_ne!(first, second);
        assert_eq!(second, pick(fnv1a64("x").wrapping_add(1)));
    }

    #[test]
    fn name_survives_reload_even_if_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let name = {
            let mut book = NameBook::load(dir.path());
            let (name, saved) = book.assign("x", &HashSet::new(), t(1));
            saved.unwrap();
            name
        };
        let used = HashSet::from([name.clone()]);
        let again = NameBook::load(dir.path()).assign("x", &used, t(9)).0;
        assert_eq!(again, name);
    }

    #[test]
    fn corrupt_files_start_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("names.json"), "{깨짐").unwrap();
        std::fs::write(dir.path().join("rooms.json"), "").unwrap();
        let (name, _) = NameBook::load(dir.path()).assign("x", &HashSet::new(), t(1));
        assert!(!name.is_empty());
        let r = RoomBook::load(dir.path()).room_of("s", &win(), t(1));
        assert_eq!(r.number / 100, 1);
    }

    #[test]
    fn save_failure_keeps_memory() {
        // 폴더 자리에 파일이 있으면 저장이 실패한다.
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocked");
        std::fs::write(&blocker, "x").unwrap();
        let mut book = NameBook::load(&blocker);
        let (name, saved) = book.assign("x", &HashSet::new(), t(1));
        assert!(saved.is_err());
        let used = HashSet::from([name.clone()]);
        assert_eq!(book.assign("x", &used, t(2)).0, name);
    }

    #[test]
    fn room_starts_at_hash_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let a = RoomBook::load(dir.path()).room_of("sess", &win(), t(1));
        a.saved.unwrap();
        let want = 100 + (fnv1a64("sess") % 99) as u32 + 1;
        assert_eq!(a.number, want);
        assert!(!a.reused);
        let b = RoomBook::load(dir.path()).room_of("sess", &win(), t(3));
        assert_eq!(b.number, want);
    }

    #[test]
    fn room_collision_takes_next_free() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = RoomBook::load(dir.path());
        let first = book.room_of("a", &win(), t(1)).number;
        let second = book.room_of(&same_start("a"), &win(), t(1)).number;
        assert_ne!(first, second);
        let next = if first % 100 == 99 { 101 } else { first + 1 };
        assert_eq!(second, next);
    }

    #[test]
    fn room_search_wraps_after_99() {
        let mut book = RoomBook::load(tempfile::tempdir().unwrap().path());
        let k = (0..)
            .map(|i| format!("k{i}"))
            .find(|k| fnv1a64(k) % 99 == 98)
            .unwrap();
        book.data.rooms.insert(
            "squatter".into(),
            RoomEntry {
                number: 199,
                last_seen: t(1).to_rfc3339(),
            },
        );
        let r = book.room_of(&k, &win(), t(1));
        assert_eq!(r.number, 101);
    }

    #[test]
    fn room_reclaimed_after_seven_days() {
        let dir = tempfile::tempdir().unwrap();
        let old = RoomBook::load(dir.path())
            .room_of("old", &win(), t(1))
            .number;
        let rival = same_start("old");
        // 정확히 7일째는 아직 점유 중
        let mut kept = RoomBook::load(dir.path());
        assert_ne!(kept.room_of(&rival, &win(), t(8)).number, old);
        // 7일을 넘기면 회수되어 같은 시작 번호를 받는다
        let dir2 = tempfile::tempdir().unwrap();
        RoomBook::load(dir2.path()).room_of("old", &win(), t(1));
        let got = RoomBook::load(dir2.path()).room_of(&rival, &win(), t(9));
        assert_eq!(got.number, old);
    }

    #[test]
    fn returning_team_after_reclaim_gets_new_room() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = RoomBook::load(dir.path());
        let first = book.room_of("old", &win(), t(1)).number;
        let squatter = same_start("old");
        assert_eq!(book.room_of(&squatter, &win(), t(10)).number, first);
        assert_ne!(book.room_of("old", &win(), t(10)).number, first);
    }

    #[test]
    fn floor_over_99_teams_reuses_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = RoomBook::load(dir.path());
        for i in 0..99 {
            assert!(!book.room_of(&format!("s{i}"), &win(), t(1)).reused);
        }
        let numbers: HashSet<_> = book.data.rooms.values().map(|r| r.number).collect();
        assert_eq!(numbers.len(), 99);
        let r = book.room_of("s99", &win(), t(1));
        assert!(r.reused);
        assert!((101..=199).contains(&r.number));
    }

    #[test]
    fn floors_follow_first_seen_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = RoomBook::load(dir.path());
        assert_eq!(book.floor_of(&wsl("Rocky")).0, 2);
        assert_eq!(book.floor_of(&win()).0, 1);
        assert_eq!(book.floor_of(&wsl("Alpine")).0, 3);
        let mut again = RoomBook::load(dir.path());
        assert_eq!(again.floor_of(&wsl("Alpine")).0, 3);
        assert_eq!(again.floor_of(&wsl("Rocky")).0, 2);
        assert_eq!(again.room_of("s", &wsl("Alpine"), t(1)).number / 100, 3);
    }

    #[test]
    fn unreadable_names_file_is_not_overwritten() {
        // 파일 자리에 폴더가 있어 읽기가 NotFound 가 아닌 오류로 실패한다.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("names.json")).unwrap();
        let mut book = NameBook::load(dir.path());
        let (name, saved) = book.assign("x", &HashSet::new(), t(1));
        assert!(!name.is_empty());
        let err = saved.unwrap_err();
        assert!(err.to_string().contains("쓰지 않음"), "{err}");
    }

    #[test]
    fn unreadable_rooms_file_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("rooms.json")).unwrap();
        let mut book = RoomBook::load(dir.path());
        let r = book.room_of("s", &win(), t(1));
        assert_eq!(r.number / 100, 1);
        let err = r.saved.unwrap_err();
        assert!(err.to_string().contains("쓰지 않음"), "{err}");
    }

    #[test]
    fn working_team_keeps_room_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let old = RoomBook::load(dir.path())
            .room_of("old", &win(), t(1))
            .number;
        // 6일 뒤, 다시 6일 뒤(처음부터 12일). 매번 껐다 켠다.
        let day6 = RoomBook::load(dir.path()).room_of("old", &win(), t(7));
        day6.saved.unwrap();
        assert_eq!(day6.number, old);
        // 12일째에 같은 시작 번호의 다른 팀이 먼저 오면 비켜 가야 한다(회수되지 않았다).
        let rival = RoomBook::load(dir.path()).room_of(&same_start("old"), &win(), t(13));
        assert_ne!(rival.number, old);
        let day12 = RoomBook::load(dir.path()).room_of("old", &win(), t(13));
        assert_eq!(day12.number, old);
    }

    #[test]
    fn windows_is_floor_one_whatever_its_id() {
        // 판정은 id 문자열이 아니라 HostKind 로 한다. discover 의 id 표기가 바뀌어도 1층이다.
        let host = Host {
            id: "win-main".into(),
            ..win()
        };
        let mut book = RoomBook::load(tempfile::tempdir().unwrap().path());
        assert_eq!(book.floor_of(&host).0, 1);
        assert_eq!(book.floor_of(&wsl("Rocky")).0, 2);
    }
}
