//! 기동 단계 진행 (PLAN 3.8 로딩 화면, 4.8 `startup-progress`)
//!
//! 이벤트 하나는 로딩 화면의 한 줄이다. 단계 본줄은 `Running` 에서 `Done` 이나 `Error` 로 끝나고,
//! 단계 밑에 붙는 알림 줄(`WslOff`, `WslFail`, `SettingsBroken`)은 summary 종류로 구분하며 본줄의 상태를 바꾸지 않는다.
//! 알림 줄의 상태는 모양이다: 꺼진 WSL 은 경고가 아니라 `Done`(소등), 접근 실패는 `Warn`.
//! 프론트가 늦게 구독해도 따라잡도록 마지막 상태를 모아 두었다가 `get_startup_progress` 로 돌려준다 (4.3 시작 순서와 같은 이유).

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::discover::wsl::WslDistro;
use crate::discover::{Host, HostKind, Site, windows_host};
use crate::office::world::OfficeWorld;
use crate::office::{Rank, TeamPhase};

pub const EVENT: &str = "startup-progress";

/// 이 안에 수정된 팀을 "불 켜진 팀"으로 센다. 읽기 전이라 2.4 의 퇴근 문턱(30분)을 mtime 으로 어림한다
const LIT_WITHIN: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub enum StepState {
    Running,
    Done,
    Warn,
    // 단계가 끝까지 못 간 경우. 일부만 실패한 Warn 과 줄 색이 다르다
    Error,
}

/// 문장 틀은 `phrases.ko.json` 의 loading 이 갖고 프론트가 이 재료로 채운다
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind")]
pub enum StartupSummary {
    None,
    /// 2단계: 후보지 수와 이름
    Candidates {
        n: u32,
        hosts: Vec<String>,
    },
    /// 3단계: 임대한 건물. 동(A~F)은 첫 레코드를 읽어야 아는 값이라 호스트와 제조사로 부른다
    Rented {
        buildings: Vec<String>,
    },
    /// 3단계 실패: 세션 폴더를 하나도 못 찾음 (3.9)
    NoFolders,
    /// 4단계: 불 켜진 팀 수
    Teams {
        n: u32,
    },
    /// 5단계: 입사 수와 직급별 수
    Hired {
        n: u32,
        bujang: u32,
        gwajang: u32,
        daeri: u32,
        sawon: u32,
    },
    /// 알림 줄: 꺼진 배포판 (3.9 "소등")
    WslOff {
        floor: u32,
        distro: String,
    },
    /// 알림 줄: 켜져 있는데 응답 없음
    WslFail {
        floor: u32,
        distro: String,
    },
    /// 알림 줄: 설정 파일 손상 (3.9)
    SettingsBroken,
}

impl StartupSummary {
    fn is_note(&self) -> bool {
        matches!(
            self,
            Self::WslOff { .. } | Self::WslFail { .. } | Self::SettingsBroken
        )
    }
}

/// 오래 걸리는 단계의 진짜 원인 (2초 넘으면 화면에 나온다)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind")]
pub enum StartupDetail {
    /// 5단계: 세션 파일 total 개 중 done 개
    Files { done: u32, total: u32 },
    /// 4단계: 이 배포판의 공유가 응답하지 않는다
    Wsl { distro: String },
}

/// `startup-progress` 페이로드
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StartupProgressDto {
    /// 1..7. 0단계(규모 선택)는 프론트 몫이다
    pub step: u8,
    pub state: StepState,
    pub summary: StartupSummary,
    pub detail: Option<StartupDetail>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 진행 상황 기록과 발행. 발행은 주입한다 (앱은 emit, 테스트는 모음)
pub struct Startup {
    log: Mutex<Vec<StartupProgressDto>>,
    emit: Box<dyn Fn(&StartupProgressDto) + Send + Sync>,
}

impl Startup {
    pub fn new(emit: impl Fn(&StartupProgressDto) + Send + Sync + 'static) -> Self {
        Self {
            log: Mutex::new(Vec::new()),
            emit: Box::new(emit),
        }
    }

    pub fn report(
        &self,
        step: u8,
        state: StepState,
        summary: StartupSummary,
        detail: Option<StartupDetail>,
    ) {
        let dto = StartupProgressDto {
            step,
            state,
            summary,
            detail,
        };
        {
            let mut log = lock(&self.log);
            // 본줄은 단계마다 하나, 알림 줄은 같은 것끼리 하나. 진행 중 갱신이 쌓이지 않게 덮는다
            let same = |d: &StartupProgressDto| {
                d.step == step
                    && if dto.summary.is_note() {
                        d.summary == dto.summary
                    } else {
                        !d.summary.is_note()
                    }
            };
            match log.iter().position(same) {
                Some(i) => log[i] = dto.clone(),
                None => log.push(dto.clone()),
            }
        }
        (self.emit)(&dto);
    }

    fn begin(&self, step: u8) {
        self.report(step, StepState::Running, StartupSummary::None, None);
    }

    /// 단계 끝이나 알림 줄 (진행 중 원인 없음)
    fn put(&self, step: u8, state: StepState, summary: StartupSummary) {
        self.report(step, state, summary, None);
    }

    /// 지금까지의 단계 본줄과 알림 줄. 늦게 구독한 프론트가 순서대로 다시 적용한다
    pub fn progress(&self) -> Vec<StartupProgressDto> {
        lock(&self.log).clone()
    }
}

#[tauri::command]
pub fn get_startup_progress(startup: State<'_, Arc<Startup>>) -> Vec<StartupProgressDto> {
    startup.progress()
}

#[derive(Default)]
struct WatchState {
    /// 호스트 → (나열된 세션 파일 수, 팀마다 파일 하나의 수정 시각).
    /// 경로는 호스트 스레드만 건드린다. 기동 스레드가 WSL 경로에 직접 닿으면 꺼진 배포판을 깨우거나 멈춘 공유에 막힌다
    listed: BTreeMap<String, (usize, Vec<SystemTime>)>,
    /// 지금까지 읽은 세션 파일 수
    read: usize,
    finished: BTreeSet<String>,
}

/// 호스트 스레드의 첫 Scan 이 알리는 곳 (4단계 나열, 5단계 읽기)
#[derive(Default)]
pub struct ScanWatch {
    state: Mutex<WatchState>,
    changed: Condvar,
}

impl ScanWatch {
    pub fn listed(&self, host: &str, files: usize, mtimes: Vec<SystemTime>) {
        lock(&self.state)
            .listed
            .insert(host.to_string(), (files, mtimes));
        self.changed.notify_all();
    }

    pub fn read(&self, files: usize) {
        lock(&self.state).read += files;
        self.changed.notify_all();
    }

    pub fn finished(&self, host: &str) {
        lock(&self.state).finished.insert(host.to_string());
        self.changed.notify_all();
    }

    /// `ready` 가 참이 되면 true, `limit` 이 지나면 false.
    /// 기다리는 동안 `tick` 마다 한 번(읽기가 아무리 잦아도 그 이상은 아님) `detail` 을 `report` 로 낸다
    fn wait(
        &self,
        limit: Instant,
        tick: Duration,
        ready: impl Fn(&WatchState) -> bool,
        detail: impl Fn(&WatchState) -> Option<StartupDetail>,
        report: impl Fn(StartupDetail),
    ) -> bool {
        let mut last = Instant::now();
        let mut g = lock(&self.state);
        loop {
            if ready(&g) {
                return true;
            }
            let now = Instant::now();
            if now >= limit {
                return false;
            }
            if now - last >= tick {
                last = now;
                let d = detail(&g);
                // emit 이 호스트 스레드의 나열·읽기 보고를 막지 않게 잠금 밖에서 낸다
                drop(g);
                if let Some(d) = d {
                    report(d);
                }
                g = lock(&self.state);
                continue;
            }
            let until = (limit - now).min(tick - (now - last));
            g = self
                .changed
                .wait_timeout(g, until)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// 기동이 바깥 세계에 하는 일. 실제 앱은 lib.rs, 테스트는 가짜 Site·가짜 Probe 로 바꿔 끼운다
pub trait Boot {
    /// 규모를 고를 때까지 기다린다 (3.8 "고르기 전에는 진행하지 않는다"). 이미 있으면 바로 돌아온다
    fn wait_for_scale(&self);
    /// 2단계: Windows 의 Site 와 WSL 배포판 (꺼진 배포판 포함)
    fn discover(&self) -> (Vec<Site>, Vec<WslDistro>);
    /// 호스트 스레드와 Probe 를 띄운다. 첫 Scan 마다 `watch` 에 알려야 한다
    fn spawn_hosts(&self, sites: Vec<Site>, distros: Vec<WslDistro>, watch: Arc<ScanWatch>);
    /// 6단계: 사이드바 구성
    fn sidebar(&self);
    /// 7단계: 발행 시작
    fn publish(&self);
}

pub struct Limits {
    /// 4~5단계가 호스트를 기다리는 최대 시간 (둘이 마감 하나를 나눠 쓴다). 지나면 그 호스트는 "응답 없음"으로 두고 진행한다
    pub step: Duration,
    /// 진행 상황을 내는 최소 간격 (4.8 "1초에 한 번 이하")
    pub tick: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            step: Duration::from_secs(30),
            tick: Duration::from_secs(1),
        }
    }
}

/// 기다림 하나의 마감과 진행 보고 간격
#[derive(Clone, Copy)]
struct Until {
    deadline: Instant,
    tick: Duration,
}

/// 기다리는 호스트 하나
#[derive(Clone)]
struct Expect {
    id: String,
    /// WSL 이면 배포판 이름
    distro: Option<String>,
    floor: u32,
}

fn host_name(h: &Host) -> String {
    match &h.kind {
        HostKind::Windows => "Windows".into(),
        HostKind::Wsl { distro } => format!("WSL {distro}"),
    }
}

/// 호스트들이 `ready` 가 될 때까지 기다린다. 시간이 다 되면 WSL 은 응답 없음 알림 줄을 남기고,
/// 어느 호스트든 더는 기다리지 않는다 (멈춘 공유 하나가 로딩 화면을 붙잡지 않게)
fn await_hosts(
    s: &Startup,
    watch: &ScanWatch,
    waits: &mut Vec<Expect>,
    step: u8,
    until: Until,
    ready: fn(&WatchState, &str) -> bool,
    detail: fn(&WatchState, &[Expect]) -> Option<StartupDetail>,
) {
    let ok = watch.wait(
        until.deadline,
        until.tick,
        |st| waits.iter().all(|e| ready(st, &e.id)),
        |st| detail(st, waits),
        |d| s.report(step, StepState::Running, StartupSummary::None, Some(d)),
    );
    if ok {
        return;
    }
    let late: Vec<Expect> = {
        let st = lock(&watch.state);
        waits
            .iter()
            .filter(|e| !ready(&st, &e.id))
            .cloned()
            .collect()
    };
    for e in &late {
        if let Some(distro) = &e.distro {
            s.put(
                step,
                StepState::Warn,
                StartupSummary::WslFail {
                    floor: e.floor,
                    distro: distro.clone(),
                },
            );
        }
    }
    waits.retain(|e| !late.iter().any(|l| l.id == e.id));
}

fn listed_or_finished(st: &WatchState, id: &str) -> bool {
    st.listed.contains_key(id) || st.finished.contains(id)
}

/// 아직 안 나열한 WSL 배포판 하나 (4단계의 원인)
fn pending_wsl(st: &WatchState, waits: &[Expect]) -> Option<StartupDetail> {
    waits
        .iter()
        .find(|e| !listed_or_finished(st, &e.id))
        .and_then(|e| e.distro.clone())
        .map(|distro| StartupDetail::Wsl { distro })
}

fn read_progress(st: &WatchState, _: &[Expect]) -> Option<StartupDetail> {
    let total: usize = st.listed.values().map(|(n, _)| n).sum();
    Some(StartupDetail::Files {
        done: st.read as u32,
        total: total as u32,
    })
}

/// 불 켜진 팀 수: 팀마다 파일 하나의 수정 시각이 문턱 안인 것
fn lit_teams(watch: &ScanWatch) -> u32 {
    let now = SystemTime::now();
    lock(&watch.state)
        .listed
        .values()
        .flat_map(|(_, m)| m)
        .filter(|t| now.duration_since(**t).is_ok_and(|age| age <= LIT_WITHIN))
        .count() as u32
}

/// 퇴근하지 않은 팀의 직원 수와 직급별 수
fn hired(world: &Mutex<OfficeWorld>) -> StartupSummary {
    let w = lock(world);
    let mut by = [0u32; 4];
    for e in w
        .teams
        .values()
        .filter(|t| t.phase != TeamPhase::Off)
        .flat_map(|t| &t.employees)
        .filter(|e| e.retired_at.is_none())
    {
        by[match e.rank {
            Rank::Bujang => 0,
            Rank::Gwajang => 1,
            Rank::Daeri => 2,
            Rank::Sawon => 3,
        }] += 1;
    }
    StartupSummary::Hired {
        n: by.iter().sum(),
        bujang: by[0],
        gwajang: by[1],
        daeri: by[2],
        sawon: by[3],
    }
}

/// 2~7단계. 1단계(설정 읽기)는 창이 뜨기 전 setup 이 했다.
/// 층 번호는 이번에 찾은 배포판 순서(2, 3 ...)다. rooms.json 의 층과 다를 수 있다 (ponytail: ipc 가 층을 내주면 맞춘다)
pub fn run(s: &Startup, boot: &dyn Boot, world: &Mutex<OfficeWorld>, lim: &Limits) {
    boot.wait_for_scale();
    let until = Until {
        deadline: Instant::now() + lim.step,
        tick: lim.tick,
    };
    s.begin(2);
    let (sites, distros) = boot.discover();
    // 켜져 있고 세션 폴더가 있는 배포판만 후보지다. 꺼진 배포판은 3단계의 소등 줄이 말한다
    let mut hosts = vec![host_name(&windows_host())];
    hosts.extend(
        distros
            .iter()
            .filter(|d| d.running == Some(true) && !d.sites.is_empty())
            .map(|d| host_name(&d.host)),
    );
    s.put(
        2,
        StepState::Done,
        StartupSummary::Candidates {
            n: hosts.len() as u32,
            hosts,
        },
    );

    s.begin(3);
    let mut waits = vec![Expect {
        id: windows_host().id,
        distro: None,
        floor: 1,
    }];
    for (floor, d) in (2..).zip(&distros) {
        let HostKind::Wsl { distro } = &d.host.kind else {
            continue;
        };
        let distro = distro.clone();
        match d.running {
            Some(false) => s.put(3, StepState::Done, StartupSummary::WslOff { floor, distro }),
            None => s.put(
                3,
                StepState::Warn,
                StartupSummary::WslFail { floor, distro },
            ),
            Some(true) if !d.sites.is_empty() => waits.push(Expect {
                id: d.host.id.clone(),
                distro: Some(distro),
                floor,
            }),
            // 켜져 있지만 세션 폴더가 없는 배포판(docker-desktop 등)은 건물이 아니다
            Some(true) => {}
        }
    }
    let buildings: Vec<String> = sites
        .iter()
        .chain(distros.iter().flat_map(|d| &d.sites))
        .map(|site| format!("{} {}", host_name(&site.host), site.vendor.label()))
        .collect();
    if buildings.is_empty() {
        s.put(3, StepState::Error, StartupSummary::NoFolders);
    } else {
        s.put(3, StepState::Done, StartupSummary::Rented { buildings });
    }

    s.begin(4);
    let watch = Arc::new(ScanWatch::default());
    boot.spawn_hosts(sites, distros, watch.clone());
    await_hosts(
        s,
        &watch,
        &mut waits,
        4,
        until,
        listed_or_finished,
        pending_wsl,
    );
    s.put(
        4,
        StepState::Done,
        StartupSummary::Teams {
            n: lit_teams(&watch),
        },
    );

    s.begin(5);
    await_hosts(
        s,
        &watch,
        &mut waits,
        5,
        until,
        |st, id| st.finished.contains(id),
        read_progress,
    );
    s.put(5, StepState::Done, hired(world));

    s.begin(6);
    boot.sidebar();
    s.put(6, StepState::Done, StartupSummary::None);

    s.begin(7);
    boot.publish();
    s.put(7, StepState::Done, StartupSummary::None);
}

/// `run` 이 패닉해도 프론트에 끝이 가게 한다: 끝나지 못한 단계를 Error 로 낸다
pub fn run_guarded(s: &Startup, boot: &dyn Boot, world: &Mutex<OfficeWorld>, lim: &Limits) {
    if catch_unwind(AssertUnwindSafe(|| run(s, boot, world, lim))).is_ok() {
        return;
    }
    let ended: BTreeSet<u8> = s
        .progress()
        .iter()
        .filter(|d| !d.summary.is_note() && matches!(d.state, StepState::Done | StepState::Error))
        .map(|d| d.step)
        .collect();
    for step in (2..=7).filter(|st| !ended.contains(st)) {
        s.put(step, StepState::Error, StartupSummary::None);
    }
}

#[cfg(test)]
mod tests;
