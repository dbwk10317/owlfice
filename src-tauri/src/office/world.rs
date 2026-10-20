//! 팀을 사무실로 묶고 변경 번호를 붙인다 (PLAN 4.7.9, 4.3)

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, FixedOffset, Utc};

use super::names::NameBook;
use super::{Confidence, Cue, Pose, Room, Team, TeamPhase};
use crate::discover::Vendor;
use crate::parse::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Large,
    Small,
}

/// 같은 원시 세션 id 가 다른 호스트·제조사에서 와도 겹치지 않게 (4.7.9).
/// product 는 첫 레코드를 읽어야 알아서 파일을 찾은 시점에 아는 vendor 로 둔다
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TeamKey {
    pub host: String,
    pub vendor: Vendor,
    pub session: String,
}

/// 대기업은 팀마다, 중소기업은 제조사마다 사무실 하나 (4.7.9)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OfficeId {
    Team(TeamKey),
    Vendor(Vendor),
}

pub struct OfficeWorld {
    pub teams: HashMap<TeamKey, Team>,
    pub scale: Scale,
    pub selected: Option<OfficeId>,
    /// 목록이 바뀔 때마다 +1 (4.3)
    pub list_seq: u64,
    /// 사무실 상태가 바뀔 때마다 그 사무실만 +1. 전역 번호 하나를 두 스트림이 나누면 같은 변경의 둘째 이벤트가 버려진다 (4.3)
    pub state_seq: HashMap<OfficeId, u64>,
    /// 야근 판정 시간대 (4.7.2)
    pub local: FixedOffset,
    names: NameBook,
}

/// 목록(office-list)에 보이는 값. 이것이 바뀌면 list_seq 를 올린다
type ListView = (TeamPhase, String, usize, bool);

/// 사무실 상태(office-state)에 보이는 값의 요약. 이것이 바뀌면 state_seq 를 올린다
type StateView = (
    TeamPhase,
    Vec<(String, Room, String, Pose, Confidence, bool)>,
    bool,
);

fn list_view(t: &Team) -> ListView {
    (
        t.phase,
        t.title.clone(),
        t.employees.len(),
        t.incident.is_some(),
    )
}

fn state_view(t: &Team) -> StateView {
    let emps = t
        .employees
        .iter()
        .map(|e| {
            (
                e.id.clone(),
                e.activity.room,
                e.activity.sentence.clone(),
                e.activity.pose,
                e.activity.confidence,
                e.retired_at.is_some(),
            )
        })
        .collect();
    (t.phase, emps, t.incident.is_some())
}

impl OfficeWorld {
    pub fn new(scale: Scale, names: NameBook, local: FixedOffset) -> Self {
        Self {
            teams: HashMap::new(),
            scale,
            selected: None,
            list_seq: 0,
            state_seq: HashMap::new(),
            local,
            names,
        }
    }

    pub fn office_of(&self, key: &TeamKey) -> OfficeId {
        match self.scale {
            Scale::Large => OfficeId::Team(key.clone()),
            Scale::Small => OfficeId::Vendor(key.vendor),
        }
    }

    /// 새 세션·Reset 뒤 처음부터 다시 만든 팀을 통째로 넣는다. 재구성은 조용해서 cue 가 없다 (4.3)
    pub fn put_team(&mut self, key: TeamKey, team: Team, now: DateTime<Utc>) {
        self.teams.insert(key.clone(), team);
        self.name_employees(&key, now);
        self.bump(&key, true);
    }

    pub fn remove_team(&mut self, key: &TeamKey) {
        if self.teams.remove(key).is_some() {
            self.list_seq += 1;
        }
    }

    /// 실행 중 append 된 이벤트를 적용한다 (4.3). 잠금 안에서는 이것만 한다
    pub fn apply(&mut self, key: &TeamKey, events: &[Event], now: DateTime<Utc>) -> Vec<Cue> {
        let Some(team) = self.teams.get_mut(key) else {
            return Vec::new();
        };
        let (list, state) = (list_view(team), state_view(team));
        let mut cues = Vec::new();
        for ev in events {
            team.apply(ev, &mut cues);
        }
        let list_changed = list != list_view(team);
        let changed = list_changed || state != state_view(team) || !cues.is_empty();
        self.name_employees(key, now);
        if changed {
            self.bump(key, list_changed);
        }
        cues
    }

    /// 시간 전이 (4.7.2). 바뀐 팀의 사무실만 번호를 올린다
    pub fn tick(&mut self, now: DateTime<Utc>) -> Vec<(OfficeId, Cue)> {
        let mut out = Vec::new();
        let mut bumps = Vec::new();
        for (key, team) in &mut self.teams {
            let (list, state) = (list_view(team), state_view(team));
            let mut cues = Vec::new();
            team.tick(now, self.local, &mut cues);
            let list_changed = list != list_view(team);
            if list_changed || state != state_view(team) || !cues.is_empty() {
                bumps.push((key.clone(), list_changed));
            }
            let office = match self.scale {
                Scale::Large => OfficeId::Team(key.clone()),
                Scale::Small => OfficeId::Vendor(key.vendor),
            };
            out.extend(cues.into_iter().map(|c| (office.clone(), c)));
        }
        for (key, list_changed) in bumps {
            self.bump(&key, list_changed);
        }
        out
    }

    /// 이름이 빈 직원에게 이름을 준다. 같은 사무실 안에서 겹치지 않아야 해서 팀이 아니라 여기서 정한다 (4.7.7)
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
            // 저장 실패는 이름을 메모리에만 남긴다 (4.7.7). 운영 로그는 T12 채우기에서
            let (name, _saved) = self.names.assign(&e.id, &in_use, now);
            in_use.insert(name.clone());
            e.name = name;
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
