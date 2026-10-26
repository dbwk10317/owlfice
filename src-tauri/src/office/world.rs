//! 팀을 사무실로 묶고 변경 번호를 붙인다 (docs/CONTRACTS.md C8·C4)

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, FixedOffset, Utc};

use super::names::{NameBook, NamesSave};
use super::work::WorkType;
use super::{Cue, Record, Team, TeamPhase};
use crate::diag::Diag;
use crate::discover::{Host, Product, Vendor};
use crate::parse::{Confidence, Event, EventKind, Unhandled};

/// 비교용 복제본이 기록 자리에 두는 빈 기록
static NO_RECORD: LazyLock<Arc<Record>> = LazyLock::new(Arc::default);

/// 개발실 6 + 임시 책상 4 (design/HANDOFF.md 6.5절)
const SEATS: u8 = 10;
/// 팀장석 4, 그다음 간이 책상 2 (design/HANDOFF.md 6.5절)
const DESKS: u8 = 4;
const SPARES: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub enum Scale {
    Large,
    Small,
}

/// 같은 원시 세션 id 가 다른 호스트·제조사에서 와도 겹치지 않게 (docs/CONTRACTS.md C8).
/// product 는 첫 레코드를 읽어야 알아서 파일을 찾은 시점에 아는 vendor 로 둔다
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TeamKey {
    pub host: String,
    pub vendor: Vendor,
    pub session: String,
}

/// 대기업은 팀마다, 중소기업은 제조사마다 사무실 하나 (docs/CONTRACTS.md C8)
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OfficeId {
    Team(TeamKey),
    Vendor(Vendor),
}

pub fn office_for(scale: Scale, key: &TeamKey) -> OfficeId {
    match scale {
        Scale::Large => OfficeId::Team(key.clone()),
        Scale::Small => OfficeId::Vendor(key.vendor),
    }
}

pub struct OfficeWorld {
    pub teams: HashMap<TeamKey, Team>,
    pub scale: Scale,
    pub selected: Option<OfficeId>,
    /// 목록이 바뀔 때마다 +1 (docs/CONTRACTS.md C4)
    pub list_seq: u64,
    /// 사무실 상태가 바뀔 때마다 그 사무실만 +1. 전역 번호 하나를 두 스트림이 나누면 같은 변경의 둘째 이벤트가 버려진다 (docs/CONTRACTS.md C4)
    pub state_seq: HashMap<OfficeId, u64>,
    /// 의심(통신 지연) 상태인 호스트. 목록에 S46 점선으로 나간다 (docs/CONTRACTS.md C5)
    pub suspect_hosts: BTreeSet<String>,
    pub read_failed_teams: BTreeSet<TeamKey>,
    // 재구성은 과거 로그를 읽으므로 종료 관측은 원본 Team 밖에서 보존한다.
    registry_ended: HashMap<TeamKey, DateTime<Utc>>,
    /// 야근 판정 시간대 (docs/CONTRACTS.md C8)
    pub local: FixedOffset,
    /// 아직 프론트에 보내지 않은 연출 신호와 그 팀. 상태 번호를 올린 같은 잠금 안에서 넣고 발행기가 비운다 (docs/CONTRACTS.md C9).
    /// 따로 넣으면 발행기가 그 사이에 돌아 번호만 먼저 나가고 연출이 늦게 붙는다
    pub outbox: Vec<(OfficeId, TeamKey, Cue)>,
    /// 상태 기계가 낸 미대응 기록(Cue::Unhandled)과 그 팀의 동. 프론트로 가지 않고 `drain_unhandled` 가 diag 에 넘긴다 (docs/CONTRACTS.md C11)
    unhandled: Vec<(Product, Unhandled)>,
    /// 호스트 id → Host. 호스트 스레드가 만들어질 때 등록한다. 층 번호·배지가 종류를 본다
    pub hosts: HashMap<String, Host>,
    /// 조용히 다시 만들었거나 규모를 바꾼 뒤 아직 발행하지 않은 사무실. 다음 office-state 가 rebuilt 로 나간다 (docs/CONTRACTS.md C4, docs/CONTRACTS.md C8)
    pub rebuilt: HashSet<OfficeId>,
    /// 규모를 바꾼 뒤 아직 발행하지 않은 목록 (docs/CONTRACTS.md C8)
    pub list_rebuilt: bool,
    names: NameBook,
}

/// 목록(office-list)에 보이는 값. 이것이 바뀌면 list_seq 를 올린다
type ListView = (TeamPhase, String, Product, usize, bool, WorkType);

fn list_view(t: &Team) -> ListView {
    (
        t.phase,
        t.title.clone(),
        t.product,
        t.employees.len(),
        t.incident.is_some(),
        t.work_type,
    )
}

pub(crate) fn main_activity_time<'a>(
    session: &str,
    events: impl Iterator<Item = &'a Event>,
) -> Option<DateTime<Utc>> {
    events
        .filter(|ev| {
            ev.agent_id == session
                && matches!(
                    ev.kind,
                    EventKind::UserMessage { .. }
                        | EventKind::AssistantText { .. }
                        | EventKind::ToolCall { .. }
                )
        })
        .filter_map(|ev| ev.ts)
        .max()
}

impl OfficeWorld {
    pub fn new(scale: Scale, names: NameBook, local: FixedOffset) -> Self {
        Self {
            teams: HashMap::new(),
            scale,
            selected: None,
            list_seq: 0,
            state_seq: HashMap::new(),
            suspect_hosts: BTreeSet::new(),
            read_failed_teams: BTreeSet::new(),
            registry_ended: HashMap::new(),
            local,
            outbox: Vec::new(),
            unhandled: Vec::new(),
            hosts: HashMap::new(),
            rebuilt: HashSet::new(),
            list_rebuilt: false,
            names,
        }
    }

    pub fn office_of(&self, key: &TeamKey) -> OfficeId {
        office_for(self.scale, key)
    }

    /// 처음부터 다시 만든 팀을 통째로 넣는다. 재구성은 조용해서 cue 가 없다 (docs/CONTRACTS.md C4).
    /// `quiet` 는 시작·새 세션·Reset 의 조용한 재구성이라 다음 office-state 를 rebuilt 로 낸다.
    /// 서브 파일이 새로 보여 다시 만든 것은 아니다. 진행 중 연출을 끊으면 안 된다
    /// 재구성은 마지막 이벤트 시각에서 끝나므로 여기서 now 까지 조용히 따라잡는다. 안 하면 다음 tick 이 cue 를 낸다.
    /// 다시 만든 팀도 자리는 그대로다. 바꾸면 Reset 마다 부장·직원이 자리를 옮긴다
    pub fn put_team(&mut self, key: TeamKey, mut team: Team, now: DateTime<Utc>, quiet: bool) {
        if self.projected_off(&key).is_none() {
            team.tick(now, self.local, &mut Vec::new());
        }
        let arrived = match self.teams.get(&key) {
            Some(old) => {
                team.desk_slot = old.desk_slot;
                for e in &mut team.employees {
                    e.seat = old
                        .employees
                        .iter()
                        .find(|o| o.id == e.id)
                        .and_then(|o| o.seat);
                }
                false
            }
            None => true,
        };
        self.teams.insert(key.clone(), team);
        self.sync_desks(&self.office_of(&key), arrived.then_some(&key));
        self.name_employees(&key, now);
        self.seat(&self.office_of(&key));
        self.bump(&key, true);
        if quiet {
            self.rebuilt.insert(self.office_of(&key));
        }
    }

    pub fn remove_team(&mut self, key: &TeamKey) {
        self.read_failed_teams.remove(key);
        self.registry_ended.remove(key);
        if self.teams.remove(key).is_none() {
            return;
        }
        self.sync_desks(&self.office_of(key), None);
        // 중소기업이면 같은 사무실에 남은 팀이 있어 그 사무실 상태도 바뀐다
        self.bump(key, true);
        // 마지막 팀이 나간 사무실은 선택할 것이 없다
        if let Some(sel) = &self.selected
            && !self.teams.keys().any(|k| self.office_of(k) == *sel)
        {
            self.selected = None;
        }
    }

    /// 연결·읽기 실패와 확인된 종료는 작업을 버리지 않고 화면·좌석·알림에서만 퇴근시킨다.
    pub fn projected_off(&self, key: &TeamKey) -> Option<Confidence> {
        if self.registry_ended.contains_key(key) {
            Some(Confidence::Confirmed)
        } else if self.suspect_hosts.contains(&key.host) || self.read_failed_teams.contains(key) {
            Some(Confidence::Inferred)
        } else {
            None
        }
    }

    pub fn displayed_phase(&self, key: &TeamKey) -> TeamPhase {
        if self.projected_off(key).is_some() {
            TeamPhase::Off
        } else {
            self.teams.get(key).map_or(TeamPhase::Off, |t| t.phase)
        }
    }

    pub fn registry_alive(&mut self, key: &TeamKey, alive: bool, now: DateTime<Utc>) {
        let Some(team) = self.teams.get_mut(key) else {
            return;
        };
        let ended = team.alive_in_registry && !alive;
        team.alive_in_registry = alive;
        if ended {
            self.registry_ended
                .insert(key.clone(), now.max(team.last_activity));
            self.refresh_presence(key);
        }
    }

    pub fn resume_registry(&mut self, key: &TeamKey, main_activity: Option<DateTime<Utc>>) -> bool {
        let resumed = self.registry_ended.get(key).is_some_and(|ended| {
            self.teams
                .get(key)
                .is_some_and(|team| team.alive_in_registry)
                && main_activity.is_some_and(|at| at > *ended)
        });
        if resumed {
            self.registry_ended.remove(key);
        }
        resumed
    }

    fn refresh_presence(&mut self, key: &TeamKey) {
        let office = self.office_of(key);
        self.sync_desks(&office, None);
        self.seat(&office);
        self.outbox.retain(|(_, k, _)| k != key);
        self.rebuilt.insert(office);
        self.bump(key, true);
    }

    pub fn set_read_failed(&mut self, key: &TeamKey, failed: bool) {
        if !self.teams.contains_key(key) {
            return;
        }
        let changed = if failed {
            self.read_failed_teams.insert(key.clone())
        } else {
            self.read_failed_teams.remove(key)
        };
        if changed {
            self.refresh_presence(key);
        }
    }

    /// 호스트의 의심 상태가 바뀌면 목록 번호와 그 호스트 팀들의 사무실 번호를 올린다 (S46 구름)
    pub fn set_suspect(&mut self, host: &str, on: bool) {
        let changed = if on {
            self.suspect_hosts.insert(host.to_string())
        } else {
            self.suspect_hosts.remove(host)
        };
        if changed {
            let keys: Vec<_> = self
                .teams
                .keys()
                .filter(|k| k.host == host)
                .cloned()
                .collect();
            if keys.is_empty() {
                self.list_seq += 1;
            }
            for key in keys {
                self.refresh_presence(&key);
            }
        }
    }

    /// 모아 둔 미대응 기록을 diag 에 넘긴다. 실행 중 적용(apply·tick) 뒤에 부른다.
    /// 잠금 순서는 world → diag 다
    pub fn drain_unhandled(&mut self, diag: &Mutex<Diag>, now: DateTime<Utc>) {
        if self.unhandled.is_empty() {
            return;
        }
        let mut d = diag.lock().unwrap_or_else(|e| e.into_inner());
        for (product, u) in self.unhandled.drain(..) {
            d.observe_unhandled(product, &u, now);
        }
    }

    /// 연출 신호를 그 팀의 사무실 앞으로 넣는다. 미대응 기록은 따로 모은다
    pub fn cue(&mut self, key: &TeamKey, cue: Cue) {
        match cue {
            Cue::Unhandled(u) => {
                if let Some(t) = self.teams.get(key) {
                    self.unhandled.push((t.product, u));
                }
            }
            cue if self.projected_off(key).is_none() => {
                self.outbox.push((self.office_of(key), key.clone(), cue))
            }
            _ => {}
        }
    }

    /// 실행 중 append 된 이벤트를 적용한다 (docs/CONTRACTS.md C4). 잠금 안에서는 이것만 한다.
    /// 이벤트마다 적용하기 전에 그 ts 로 tick 한다. 재구성(replay)과 같은 순서라서
    /// 몇 줄씩 묶어 읽혔든 한 줄씩 읽혔든 같은 상태가 된다 (docs/CONTRACTS.md C4 시계)
    pub fn apply(&mut self, key: &TeamKey, events: &[Event], now: DateTime<Utc>) {
        let local = self.local;
        let resumed = self.resume_registry(key, main_activity_time(&key.session, events.iter()));
        let projected = self.projected_off(key).is_some();
        let Some(team) = self.teams.get_mut(key) else {
            return;
        };
        let mut before = team.clone();
        // 기록(Arc)을 함께 쥐면 team.apply 의 make_mut 가 타임라인 전체를 이벤트마다 복사한다.
        // 기록이 바뀌면 Team.seq 도 바뀌어서 비교에서 빠져도 놓치는 변화가 없다
        for e in &mut before.employees {
            e.record = NO_RECORD.clone();
        }
        // 확정 결과 원장도 현재 이벤트가 추가되는 동안은 비교본과 공유하지 않는다.
        before.daily_events = Arc::new(Vec::new());
        let mut cues = Vec::new();
        for ev in events {
            if let Some(t) = ev.ts
                && !projected
            {
                team.tick(t, local, &mut cues);
            }
            team.apply(ev, &mut cues);
        }
        for (b, e) in before.employees.iter_mut().zip(&team.employees) {
            b.record = e.record.clone();
        }
        before.daily_events = team.daily_events.clone();
        self.settle(key, &before, now, cues.iter().any(Cue::shown));
        if resumed {
            self.refresh_presence(key);
        }
        if !projected && !resumed {
            for c in cues {
                self.cue(key, c);
            }
        }
    }

    /// 세션 등록부 값을 팀에 넣는다 (docs/CONTRACTS.md C5). 제목이 바뀌면 jsonl 제목처럼 액자를 간다 (S39)
    pub fn set_registry(&mut self, key: &TeamKey, name: Option<&str>, product: Option<Product>) {
        let Some(team) = self.teams.get_mut(key) else {
            return;
        };
        let before = list_view(team);
        let mut cues = Vec::new();
        if !team.set_registry(name, product, &mut cues) {
            return;
        }
        let list_changed = list_view(team) != before;
        self.bump(key, list_changed);
        for c in cues {
            self.cue(key, c);
        }
    }

    /// 서브 파일·meta.json 이 사라진 직원을 퇴장시킨다 (docs/CONTRACTS.md C6 신규 세션 발견)
    pub fn retire_employee(&mut self, key: &TeamKey, agent_id: &str, at: DateTime<Utc>) {
        let Some(team) = self.teams.get_mut(key) else {
            return;
        };
        let before = team.clone();
        let mut cues = Vec::new();
        team.retire_employee(agent_id, at, &mut cues);
        self.settle(key, &before, at, !cues.is_empty());
        for c in cues {
            self.cue(key, c);
        }
    }

    /// 바뀐 팀에 이름·자리를 주고 번호를 올린다. 보낼 상태 전체를 비교해 소품·실적만 바뀌어도 올린다
    fn settle(&mut self, key: &TeamKey, before: &Team, now: DateTime<Utc>, cued: bool) {
        self.name_employees(key, now);
        let desks_changed = self.sync_desks(&self.office_of(key), None);
        self.seat(&self.office_of(key));
        let Some(team) = self.teams.get(key) else {
            return;
        };
        let list_changed = list_view(before) != list_view(team);
        if list_changed || before != team || cued || desks_changed {
            self.bump(key, list_changed);
        }
    }

    /// 선택 사무실을 바꾼다. 상태는 이미 있어서 폴링 주기만 바뀐다 (docs/CONTRACTS.md C6 선택 전환).
    /// 프론트가 보낸 값이라 지금 있는 사무실만 받는다
    pub fn select(&mut self, office: Option<OfficeId>) -> bool {
        if let Some(o) = &office
            && !self.teams.keys().any(|k| self.office_of(k) == *o)
        {
            return false;
        }
        self.selected = office;
        true
    }

    /// 규모 변경 (docs/CONTRACTS.md C8). 팀 상태는 그대로 두고 배치만 다시 한다.
    /// 4·5번(연출 비우기, rebuilt 발행)은 outbox·rebuilt 를 보고 발행기가 한다
    pub fn set_scale(&mut self, scale: Scale) {
        if scale == self.scale {
            return;
        }
        let old = self.selected.take();
        self.scale = scale;
        self.selected = match old {
            Some(OfficeId::Team(k)) => Some(OfficeId::Vendor(k.vendor)),
            // 그 층에서 가장 최근 활동한 팀
            Some(OfficeId::Vendor(v)) => self
                .teams
                .iter()
                .filter(|(k, _)| k.vendor == v)
                .max_by_key(|(k, t)| (t.last_activity, *k))
                .map(|(k, _)| OfficeId::Team(k.clone())),
            None => None,
        };
        for team in self.teams.values_mut() {
            team.desk_slot = None;
            for e in &mut team.employees {
                e.seat = None;
            }
        }
        let offices: BTreeSet<OfficeId> = self.teams.keys().map(|k| self.office_of(k)).collect();
        for office in &offices {
            // 대기업에서 각자 0번이던 자리가 합쳐질 때 최근 활동한 팀부터 다시 준다
            let mut keys = self.keys_in(office);
            keys.retain(|k| self.displayed_phase(k) != TeamPhase::Off);
            keys.sort_by_key(|k| {
                (
                    std::cmp::Reverse(self.teams[k].last_activity),
                    self.teams[k].started,
                    k.clone(),
                )
            });
            for (k, slot) in keys.iter().zip(0..DESKS + SPARES) {
                if let Some(t) = self.teams.get_mut(k) {
                    t.desk_slot = Some(slot);
                }
            }
            self.seat(office);
            *self.state_seq.entry(office.clone()).or_default() += 1;
        }
        self.list_seq += 1;
        // 이전 배치를 향한 연출은 버린다 (docs/CONTRACTS.md C8)
        self.outbox.clear();
        self.rebuilt.extend(offices);
        self.list_rebuilt = true;
    }

    /// 현재 OS 시간대로 날짜·조명 표시를 다시 계산한다. 수집이 멈춘 호스트의 상태 전이는 진행하지 않는다.
    pub fn set_local_offset(&mut self, local: FixedOffset, now: DateTime<Utc>) {
        let mut changed = BTreeSet::new();
        for (key, team) in &mut self.teams {
            if team.set_local_offset(local, now) {
                changed.insert(office_for(self.scale, key));
            }
        }
        self.local = local;
        for office in changed {
            *self.state_seq.entry(office).or_default() += 1;
        }
    }

    /// 시간 전이 (docs/CONTRACTS.md C8). `host` 의 팀만 돌린다. 다른 호스트의 팀은 그 스레드가 돌리고,
    /// 의심 중인 호스트의 팀은 못 읽은 줄이 있을 수 있어 멈춘다 (docs/CONTRACTS.md C4 시계). 바뀐 팀의 사무실만 번호를 올린다
    pub fn tick(&mut self, now: DateTime<Utc>, host: &str) {
        let mut out = Vec::new();
        let mut bumps = Vec::new();
        let paused: HashSet<_> = self
            .teams
            .keys()
            .filter(|k| self.projected_off(k).is_some())
            .cloned()
            .collect();
        for (key, team) in self
            .teams
            .iter_mut()
            .filter(|(k, _)| k.host == host && !paused.contains(*k))
        {
            let before = team.clone();
            let mut cues = Vec::new();
            team.tick(now, self.local, &mut cues);
            let list_changed = list_view(&before) != list_view(team);
            if list_changed || before != *team || cues.iter().any(Cue::shown) {
                bumps.push((key.clone(), list_changed));
            }
            let office = office_for(self.scale, key);
            for c in cues {
                match c {
                    Cue::Unhandled(u) => self.unhandled.push((team.product, u)),
                    c => out.push((office.clone(), key.clone(), c)),
                }
            }
        }
        for (key, list_changed) in bumps {
            // 퇴장·퇴근한 직원의 자리를 비운다
            self.sync_desks(&self.office_of(&key), None);
            self.seat(&self.office_of(&key));
            self.bump(&key, list_changed);
        }
        self.outbox.extend(out);
    }

    /// 아직 저장하지 않은 이름. 부르는 쪽이 잠금을 놓은 뒤 쓴다 (docs/CONTRACTS.md C8)
    pub fn unsaved_names(&mut self) -> Option<NamesSave> {
        self.names.unsaved()
    }

    /// 이름이 빈 직원에게 이름을 준다. 같은 사무실 안에서 겹치지 않아야 해서 팀이 아니라 여기서 정한다 (docs/CONTRACTS.md C8).
    /// 잠금 안이라 메모리에만 배정하고, 저장은 `unsaved_names` 로 잠금 밖에서 한다
    fn name_employees(&mut self, key: &TeamKey, now: DateTime<Utc>) {
        let office = self.office_of(key);
        let mut in_use: HashSet<String> = self
            .teams
            .iter()
            .filter(|(k, _)| self.office_of(k) == office)
            .flat_map(|(_, t)| t.employees.iter().map(|e| e.name.clone()))
            .filter(|n| !n.is_empty())
            .collect();
        let Some(team) = self.teams.get_mut(key) else {
            return;
        };
        for e in team.employees.iter_mut().filter(|e| e.name.is_empty()) {
            let name = self.names.assign_unsaved(&e.id, &in_use, now);
            in_use.insert(name.clone());
            e.name = name;
        }
    }

    fn keys_in(&self, office: &OfficeId) -> Vec<TeamKey> {
        self.teams
            .keys()
            .filter(|k| self.office_of(k) == *office)
            .cloned()
            .collect()
    }

    /// 퇴근 팀은 정원을 쓰지 않는다. 기존 팀은 빈자리만 받아 정상 팀의 자리를 흔들지 않는다.
    fn sync_desks(&mut self, office: &OfficeId, arriving: Option<&TeamKey>) -> bool {
        let mut keys = self.keys_in(office);
        let before: Vec<_> = keys
            .iter()
            .map(|k| (k.clone(), self.teams[k].desk_slot))
            .collect();
        for key in &keys {
            if self.displayed_phase(key) == TeamPhase::Off
                && let Some(team) = self.teams.get_mut(key)
            {
                team.desk_slot = None;
            }
        }
        if let Some(key) = arriving
            && self.displayed_phase(key) != TeamPhase::Off
        {
            self.place_desk(key);
        }
        let used: HashSet<_> = keys
            .iter()
            .filter_map(|k| self.teams[k].desk_slot)
            .collect();
        keys.retain(|k| {
            self.displayed_phase(k) != TeamPhase::Off && self.teams[k].desk_slot.is_none()
        });
        keys.sort_by_key(|k| {
            (
                std::cmp::Reverse(self.teams[k].last_activity),
                self.teams[k].started,
                k.clone(),
            )
        });
        let free = (0..DESKS + SPARES).filter(|s| !used.contains(s));
        for (key, slot) in keys.iter().zip(free) {
            if let Some(team) = self.teams.get_mut(key) {
                team.desk_slot = Some(slot);
            }
        }
        before
            .iter()
            .any(|(k, slot)| self.teams[k].desk_slot != *slot)
    }

    /// 들어온 팀에 팀장석을 준다. 한 번 준 자리는 활동 순서로 바꾸지 않는다 (docs/CONTRACTS.md C8).
    /// 네 자리가 찼으면 새 팀을 포함해 가장 오래 조용한 팀이 간이 책상으로, 간이 책상도 찼으면 카드만.
    /// 새 팀을 포함하는 것은 시작 때 옛 세션들이 아무 순서로 들어와도 활동 중인 팀이 밀려나지 않게다
    fn place_desk(&mut self, key: &TeamKey) {
        let office = self.office_of(key);
        let others: Vec<(TeamKey, Option<u8>, DateTime<Utc>)> = self
            .keys_in(&office)
            .into_iter()
            .filter(|k| k != key)
            .map(|k| {
                let t = &self.teams[&k];
                (k, t.desk_slot, t.last_activity)
            })
            .collect();
        let used: HashSet<u8> = others.iter().filter_map(|o| o.1).collect();
        let free = |mut range: std::ops::Range<u8>| range.find(|s| !used.contains(s));
        let mut slot = free(0..DESKS);
        if slot.is_none() {
            let spare = free(DESKS..DESKS + SPARES);
            let mine = self.teams[key].last_activity;
            let quietest = others
                .iter()
                .filter(|o| o.1.is_some_and(|s| s < DESKS))
                .min_by_key(|o| (o.2, &o.0))
                .filter(|o| o.2 < mine);
            slot = match quietest {
                Some((k, s, _)) => {
                    if let Some(t) = self.teams.get_mut(k) {
                        t.desk_slot = spare;
                    }
                    *s
                }
                None => spare,
            };
        }
        if let Some(t) = self.teams.get_mut(key) {
            t.desk_slot = slot;
        }
    }

    /// 자리 없는 직원에게 빈 자리를 입사 순으로 준다. 부장(팀장석)과 퇴장·퇴근한 직원은 자리를 비운다.
    /// 개발실 좌석은 사무실의 모든 팀이 함께 쓴다 (docs/CONTRACTS.md C8)
    fn seat(&mut self, office: &OfficeId) {
        let mut keys = self.keys_in(office);
        keys.sort();
        let mut used = HashSet::new();
        let mut want = Vec::new();
        for k in &keys {
            let off = self.displayed_phase(k) == TeamPhase::Off;
            let Some(t) = self.teams.get_mut(k) else {
                continue;
            };
            for (i, e) in t.employees.iter_mut().enumerate() {
                if i == 0 || off || e.retired_at.is_some() {
                    e.seat = None;
                    continue;
                }
                match e.seat {
                    Some(s) => {
                        used.insert(s);
                    }
                    None => want.push((e.hired_at, k.clone(), i)),
                }
            }
        }
        want.sort();
        let mut free = (0..SEATS).filter(|s| !used.contains(s));
        for (_, k, i) in want {
            // 자리가 모자라면 카드로만 보인다
            let Some(s) = free.next() else {
                break;
            };
            if let Some(t) = self.teams.get_mut(&k) {
                t.employees[i].seat = Some(s);
            }
        }
    }

    fn bump(&mut self, key: &TeamKey, list_changed: bool) {
        let office = self.office_of(key);
        *self.state_seq.entry(office).or_default() += 1;
        if list_changed {
            self.list_seq += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::Product;
    use chrono::Duration;

    fn t0() -> DateTime<Utc> {
        "2026-03-02T09:00:00Z".parse().unwrap()
    }

    fn key(vendor: Vendor, session: &str) -> TeamKey {
        TeamKey {
            host: "windows".into(),
            vendor,
            session: session.into(),
        }
    }

    /// 부장 + 서브 n 명, `mins` 분에 시작하고 마지막 활동
    fn team(session: &str, subs: usize, mins: i64) -> Team {
        let at = t0() + Duration::minutes(mins);
        let mut t = Team::new(session, Product::ClaudeDesktop, at);
        for i in 0..subs {
            t.preregister(&format!("{session}-s{i}"), None, None, None, 1, at);
        }
        t
    }

    /// 사무실마다 자리·팀장석이 겹치지 않고 비어 있지 않다
    fn assert_no_overlap(w: &OfficeWorld) {
        let mut offices: Vec<OfficeId> = w.teams.keys().map(|k| w.office_of(k)).collect();
        offices.sort();
        offices.dedup();
        for o in offices {
            let teams: Vec<&Team> = w
                .teams
                .iter()
                .filter(|(k, _)| w.office_of(k) == o)
                .map(|(_, t)| t)
                .collect();
            let seats: Vec<u8> = teams
                .iter()
                .flat_map(|t| t.employees.iter().skip(1).map(|e| e.seat.unwrap()))
                .collect();
            let desks: Vec<u8> = teams.iter().filter_map(|t| t.desk_slot).collect();
            for (what, v) in [("seat", &seats), ("desk", &desks)] {
                let mut u = v.clone();
                u.sort();
                u.dedup();
                assert_eq!(u.len(), v.len(), "{o:?} {what} 겹침 {v:?}");
            }
            assert!(teams.iter().all(|t| t.employees[0].seat.is_none()));
        }
    }

    #[test]
    fn scale_round_trip_keeps_seats_and_desks_apart_and_maps_selection() {
        let mut w = OfficeWorld::new(
            Scale::Large,
            NameBook::load(tempfile::tempdir().unwrap().path()),
            FixedOffset::east_opt(0).unwrap(),
        );
        let (a, b, c) = (
            key(Vendor::Claude, "a"),
            key(Vendor::Claude, "b"),
            key(Vendor::Claude, "c"),
        );
        let x = key(Vendor::Codex, "x");
        w.put_team(a.clone(), team("a", 3, 0), t0(), true);
        w.put_team(b.clone(), team("b", 2, 10), t0(), true);
        w.put_team(c.clone(), team("c", 1, 5), t0(), true);
        w.put_team(x.clone(), team("x", 2, 1), t0(), true);
        assert_no_overlap(&w);
        // 대기업은 팀마다 0번부터
        assert_eq!(w.teams[&a].employees[1].seat, Some(0));
        assert_eq!(w.teams[&b].employees[1].seat, Some(0));
        assert!(w.select(Some(OfficeId::Team(a.clone()))));
        assert!(!w.select(Some(OfficeId::Team(key(Vendor::Claude, "없음")))));

        w.set_scale(Scale::Small);
        assert_eq!(w.selected, Some(OfficeId::Vendor(Vendor::Claude)));
        assert_no_overlap(&w);
        // 팀장석은 최근 활동 순
        let desk = |w: &OfficeWorld, k: &TeamKey| w.teams[k].desk_slot;
        assert_eq!(
            [desk(&w, &b), desk(&w, &c), desk(&w, &a)],
            [Some(0), Some(1), Some(2)]
        );
        assert_eq!(desk(&w, &x), Some(0));

        w.set_scale(Scale::Large);
        // 그 층에서 가장 최근 활동한 팀
        assert_eq!(w.selected, Some(OfficeId::Team(b.clone())));
        assert_no_overlap(&w);
        assert!(w.teams.values().all(|t| t.desk_slot == Some(0)));

        w.set_scale(Scale::Small);
        assert_no_overlap(&w);
        // 네 자리가 찼을 때 새 팀이 오면 가장 오래 조용한 팀(a)이 간이 책상으로 내려가고 자리를 넘긴다
        let d = key(Vendor::Claude, "d");
        w.put_team(d.clone(), team("d", 1, 20), t0(), true);
        assert_eq!(desk(&w, &d), Some(3));
        let e = key(Vendor::Claude, "e");
        w.put_team(e.clone(), team("e", 1, 30), t0(), true);
        assert_eq!((desk(&w, &e), desk(&w, &a)), (Some(2), Some(4)));
        // 새 팀이 가장 조용하면 자기가 간이 책상으로 간다
        let old = key(Vendor::Claude, "old");
        w.put_team(old.clone(), team("old", 0, -1), t0(), true);
        assert_eq!(desk(&w, &old), Some(5));
        assert_no_overlap(&w);
        // 다시 만든 팀은 자리를 그대로 둔다
        let before: Vec<Option<u8>> = w.teams[&b].employees.iter().map(|e| e.seat).collect();
        w.put_team(b.clone(), team("b", 2, 10), t0(), true);
        let after: Vec<Option<u8>> = w.teams[&b].employees.iter().map(|e| e.seat).collect();
        assert_eq!((before, Some(0)), (after, desk(&w, &b)));
    }

    #[test]
    fn scale_change_seats_active_teams_before_older_off_teams() {
        let mut w = OfficeWorld::new(
            Scale::Large,
            NameBook::load(tempfile::tempdir().unwrap().path()),
            FixedOffset::east_opt(0).unwrap(),
        );
        let old = key(Vendor::Claude, "old");
        let mut off = team("old", 0, 0);
        off.phase = TeamPhase::Off;
        w.put_team(old.clone(), off, t0(), true);
        let active: Vec<_> = (0..6)
            .map(|i| key(Vendor::Claude, &format!("active-{i}")))
            .collect();
        for (i, key) in active.iter().enumerate() {
            w.put_team(key.clone(), team(&key.session, 0, i as i64 + 1), t0(), true);
        }
        w.set_scale(Scale::Small);
        assert_eq!(w.teams[&old].desk_slot, None);
        assert_eq!(
            active
                .iter()
                .filter(|k| w.teams[*k].desk_slot.is_some())
                .count(),
            6
        );
        let before: Vec<_> = active.iter().map(|k| w.teams[k].desk_slot).collect();
        w.set_scale(Scale::Large);
        w.set_scale(Scale::Small);
        assert_eq!(
            active
                .iter()
                .map(|k| w.teams[k].desk_slot)
                .collect::<Vec<_>>(),
            before
        );
    }

    #[test]
    fn scale_change_keeps_recent_active_teams_when_desks_are_full() {
        let mut w = OfficeWorld::new(
            Scale::Large,
            NameBook::load(tempfile::tempdir().unwrap().path()),
            FixedOffset::east_opt(0).unwrap(),
        );
        let old = key(Vendor::Claude, "old-dozing");
        let mut dozing = team("old-dozing", 0, 0);
        dozing.phase = TeamPhase::Dozing;
        w.put_team(old.clone(), dozing, t0(), true);
        let off = key(Vendor::Claude, "off");
        let mut ended = team("off", 0, 0);
        ended.phase = TeamPhase::Off;
        w.put_team(off.clone(), ended, t0(), true);
        let active: Vec<_> = (0..6)
            .map(|i| key(Vendor::Claude, &format!("active-{i}")))
            .collect();
        for (i, key) in active.iter().enumerate() {
            w.put_team(key.clone(), team(&key.session, 0, i as i64 + 1), t0(), true);
        }
        w.set_scale(Scale::Small);
        assert_eq!(w.teams[&old].desk_slot, None);
        assert_eq!(w.teams[&off].desk_slot, None);
        assert!(active.iter().all(|k| w.teams[k].desk_slot.is_some()));
    }

    #[test]
    fn timezone_change_updates_display_without_ticking_suspect_host() {
        let mut w = OfficeWorld::new(
            Scale::Large,
            NameBook::load(tempfile::tempdir().unwrap().path()),
            FixedOffset::east_opt(0).unwrap(),
        );
        let key = key(Vendor::Claude, "t");
        w.put_team(key.clone(), team("t", 0, 0), t0(), true);
        w.set_suspect("windows", true);
        let office = OfficeId::Team(key.clone());
        let seq = w.state_seq[&office];
        let phase = w.teams[&key].phase;
        w.set_local_offset(FixedOffset::west_opt(8 * 3600).unwrap(), t0());
        assert!(w.state_seq[&office] > seq);
        assert_eq!(w.teams[&key].phase, phase);
        assert_eq!(w.teams[&key].daylight(t0()), super::super::Daylight::Night);
    }

    fn world(scale: Scale) -> OfficeWorld {
        OfficeWorld::new(
            scale,
            NameBook::load(tempfile::tempdir().unwrap().path()),
            FixedOffset::east_opt(0).unwrap(),
        )
    }

    #[test]
    fn small_start_does_not_let_off_teams_displace_six_active_teams() {
        let mut w = world(Scale::Small);
        let old: Vec<_> = ["old-a", "old-b"]
            .map(|id| {
                let key = key(Vendor::Claude, id);
                w.put_team(key.clone(), team(id, 0, -120), t0(), true);
                key
            })
            .into();
        let active: Vec<_> = (0..6)
            .map(|i| {
                let key = key(Vendor::Claude, &format!("active-{i}"));
                w.put_team(key.clone(), team(&key.session, 0, i + 1), t0(), true);
                key
            })
            .collect();
        assert_eq!(
            active
                .iter()
                .filter(|k| w.teams[*k].desk_slot.is_some())
                .count(),
            6,
            "퇴근 팀이 남아 있어도 활성 6팀은 모두 자리 필요"
        );
        assert!(
            old.iter()
                .all(|k| w.teams[k].phase == TeamPhase::Off && w.teams[k].desk_slot.is_none())
        );
        assert_no_overlap(&w);
        let before: Vec<_> = active.iter().map(|k| w.teams[k].desk_slot).collect();
        w.put_team(
            active[0].clone(),
            team(&active[0].session, 0, 100),
            t0(),
            true,
        );
        assert_eq!(
            active
                .iter()
                .map(|k| w.teams[k].desk_slot)
                .collect::<Vec<_>>(),
            before
        );
    }

    fn thinking(key: &TeamKey, at: DateTime<Utc>) -> Event {
        Event {
            ts: Some(at),
            session_id: key.session.clone(),
            agent_id: key.session.clone(),
            parent_agent: None,
            kind: crate::parse::EventKind::Thinking,
            raw_ref: crate::parse::RawRef {
                path: std::path::PathBuf::new(),
                generation: 0,
                byte_offset: 0,
                line_no: 0,
            },
        }
    }

    #[test]
    fn confirmed_exit_survives_rebuild_until_live_main_activity_resumes() {
        let mut w = world(Scale::Small);
        let k = key(Vendor::Claude, "exit");
        w.put_team(k.clone(), team("exit", 1, 0), t0(), true);
        w.registry_alive(&k, false, t0());
        assert_eq!(
            w.projected_off(&k),
            None,
            "생존 미확인은 종료 증거가 아니다"
        );
        w.registry_alive(&k, true, t0());
        let ended = t0() + Duration::seconds(10);
        w.registry_alive(&k, false, ended);
        assert_eq!(w.projected_off(&k), Some(Confidence::Confirmed));
        assert_eq!(w.displayed_phase(&k), TeamPhase::Off);
        assert_eq!(w.teams[&k].phase, TeamPhase::Working);
        assert!(w.teams[&k].desk_slot.is_none());
        assert!(w.teams[&k].employees.iter().all(|e| e.seat.is_none()));
        w.put_team(k.clone(), team("exit", 1, 0), ended, true);
        assert_eq!(w.displayed_phase(&k), TeamPhase::Off);
        w.registry_alive(&k, true, ended);
        let mut event = thinking(&k, ended);
        event.kind = EventKind::UserMessage {
            text: "재개".into(),
        };
        w.apply(&k, &[event.clone()], ended);
        assert_eq!(w.displayed_phase(&k), TeamPhase::Off);
        event.ts = Some(ended + Duration::seconds(1));
        event.agent_id = "exit-s0".into();
        w.apply(&k, &[event.clone()], ended + Duration::seconds(1));
        assert_eq!(w.displayed_phase(&k), TeamPhase::Off);
        event.agent_id = "exit".into();
        w.apply(&k, &[event], ended + Duration::seconds(1));
        assert_eq!(w.projected_off(&k), None);
        assert_eq!(w.displayed_phase(&k), TeamPhase::Working);
        assert!(w.teams[&k].desk_slot.is_some());
    }

    #[test]
    fn failed_connections_pause_work_and_restore_seats_without_losing_records() {
        for host_failure in [false, true] {
            let mut w = world(Scale::Small);
            let k = key(Vendor::Claude, "failure");
            w.put_team(k.clone(), team("failure", 1, 0), t0(), true);
            let original = w.teams[&k].clone();
            if host_failure {
                w.set_suspect("windows", true);
            } else {
                w.set_read_failed(&k, true);
            }
            w.tick(t0() + Duration::hours(1), "windows");
            let mut expected = original.clone();
            expected.desk_slot = None;
            for e in &mut expected.employees {
                e.seat = None;
            }
            assert_eq!(w.teams[&k], expected);
            assert_eq!(w.projected_off(&k), Some(Confidence::Inferred));
            if host_failure {
                w.set_suspect("windows", false);
            } else {
                w.set_read_failed(&k, false);
            }
            assert_eq!(w.teams[&k], original);
            assert_eq!(w.projected_off(&k), None);
        }
    }

    #[test]
    fn existing_off_team_gets_a_free_desk_on_append_without_moving_active_teams() {
        let mut w = world(Scale::Small);
        let returning = key(Vendor::Claude, "returning");
        w.put_team(returning.clone(), team("returning", 0, -120), t0(), true);
        let active: Vec<_> = (0..5)
            .map(|i| {
                let key = key(Vendor::Claude, &format!("active-{i}"));
                w.put_team(key.clone(), team(&key.session, 0, i + 1), t0(), true);
                key
            })
            .collect();
        w.set_scale(Scale::Large);
        w.set_scale(Scale::Small);
        assert_eq!(w.teams[&returning].desk_slot, None);
        let before: Vec<_> = active.iter().map(|k| w.teams[k].desk_slot).collect();
        let office = w.office_of(&returning);
        let seq = w.state_seq[&office];
        let at = t0() + Duration::seconds(1);
        w.apply(&returning, &[thinking(&returning, at)], at);
        assert_eq!(w.teams[&returning].phase, TeamPhase::Working);
        assert!(w.teams[&returning].desk_slot.is_some());
        assert_eq!(
            active
                .iter()
                .map(|k| w.teams[k].desk_slot)
                .collect::<Vec<_>>(),
            before
        );
        assert!(w.state_seq[&office] > seq);
        assert_no_overlap(&w);
    }

    #[test]
    fn desk_vacancies_recover_waiting_teams_after_remove_rebuild_or_tick() {
        for cause in ["remove", "rebuild", "tick"] {
            let mut w = world(Scale::Small);
            let active: Vec<_> = (0..6)
                .map(|i| {
                    let key = key(Vendor::Claude, &format!("active-{i}"));
                    w.put_team(key.clone(), team(&key.session, 0, i + 1), t0(), true);
                    key
                })
                .collect();
            let waiting = key(Vendor::Claude, "waiting");
            w.put_team(waiting.clone(), team("waiting", 0, 0), t0(), true);
            assert_eq!(w.teams[&waiting].desk_slot, None);
            let vacancy = w.teams[&active[0]].desk_slot;
            let unchanged: Vec<_> = active[1..].iter().map(|k| w.teams[k].desk_slot).collect();
            if cause == "tick" {
                let at = t0() + Duration::minutes(25);
                w.apply(&waiting, &[thinking(&waiting, at)], at);
                assert_eq!(w.teams[&waiting].desk_slot, None);
            }
            let office = w.office_of(&waiting);
            let seq = w.state_seq[&office];
            match cause {
                "remove" => w.remove_team(&active[0]),
                "rebuild" => w.put_team(
                    active[0].clone(),
                    team(&active[0].session, 0, -120),
                    t0(),
                    true,
                ),
                "tick" => w.tick(
                    t0() + Duration::minutes(31) + Duration::seconds(1),
                    "windows",
                ),
                _ => unreachable!(),
            }
            assert_eq!(w.teams[&waiting].desk_slot, vacancy, "{cause}");
            assert_eq!(
                active[1..]
                    .iter()
                    .map(|k| w.teams[k].desk_slot)
                    .collect::<Vec<_>>(),
                unchanged,
                "{cause}"
            );
            if cause != "remove" {
                assert_eq!(w.teams[&active[0]].phase, TeamPhase::Off, "{cause}");
                assert_eq!(w.teams[&active[0]].desk_slot, None, "{cause}");
            }
            assert!(w.state_seq[&office] > seq, "{cause}");
            assert_no_overlap(&w);
        }
    }

    /// cue 없이 소품(화분)만 바뀌어도 사무실 번호가 오른다
    #[test]
    fn prop_only_change_bumps_state_seq() {
        let mut w = world(Scale::Large);
        let a = key(Vendor::Claude, "a");
        w.put_team(a.clone(), team("a", 0, 0), t0(), true);
        let usage = |input_total, secs| Event {
            ts: Some(t0() + Duration::seconds(secs)),
            session_id: "a".into(),
            agent_id: "a".into(),
            parent_agent: None,
            kind: crate::parse::EventKind::TokenUsage {
                input_total,
                cache_read: 0,
                output: 0,
                context_window: Some(1000),
            },
            raw_ref: crate::parse::RawRef {
                path: std::path::PathBuf::new(),
                generation: 0,
                byte_offset: 0,
                line_no: 0,
            },
        };
        let office = w.office_of(&a);
        let seq = |w: &OfficeWorld| w.state_seq[&office];
        let s0 = seq(&w);
        w.apply(&a, &[usage(347, 1)], t0());
        let s1 = seq(&w);
        w.apply(&a, &[usage(750, 2)], t0());
        assert!(w.outbox.is_empty());
        assert_eq!(w.teams[&a].props.plant, Some(0.75));
        assert!(s0 < s1 && s1 < seq(&w));
    }

    /// 등록부가 동만 바꿔도 목록(동 표시)이 다시 나간다
    #[test]
    fn registry_building_only_change_bumps_list_seq() {
        let mut w = world(Scale::Large);
        let a = key(Vendor::Claude, "a");
        w.put_team(a.clone(), team("a", 0, 0), t0(), true);
        let list = w.list_seq;
        w.set_registry(&a, None, Some(Product::ClaudeCli));
        assert_eq!(w.teams[&a].product, Product::ClaudeCli);
        assert!(w.list_seq > list);
    }

    /// 중소기업에서 팀이 나가면 남은 팀의 사무실 상태도 바뀐다
    #[test]
    fn remove_team_bumps_shared_office_state() {
        let mut w = world(Scale::Small);
        let (a, b) = (key(Vendor::Claude, "a"), key(Vendor::Claude, "b"));
        w.put_team(a.clone(), team("a", 1, 0), t0(), true);
        w.put_team(b.clone(), team("b", 1, 1), t0(), true);
        let office = OfficeId::Vendor(Vendor::Claude);
        let (before, list) = (w.state_seq[&office], w.list_seq);
        w.remove_team(&a);
        assert!(w.state_seq[&office] > before);
        assert!(w.list_seq > list);
    }
}
