//! 창이 가려진 동안 오래 이어진 결재 대기만 알린다 (docs/CONTRACTS.md C3).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Duration, Utc};
use tauri::{AppHandle, State};
use tauri_plugin_notification::NotificationExt;

use crate::office::TeamPhase;
use crate::office::world::{OfficeWorld, TeamKey};

#[derive(Default)]
pub struct WindowVisibility(AtomicBool);

impl WindowVisibility {
    pub fn hidden(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[tauri::command]
pub fn set_window_hidden(visibility: State<'_, WindowVisibility>, hidden: bool) {
    visibility.0.store(hidden, Ordering::Relaxed);
}

struct Episode {
    since: DateTime<Utc>,
    sent: bool,
}

#[derive(Default)]
pub struct PendingNotifications {
    episodes: HashMap<(TeamKey, String), Episode>,
}

impl PendingNotifications {
    /// 화면 선택과 무관하게 모든 직원의 needs_boss 전이를 관찰한다.
    pub fn poll(
        &mut self,
        world: &OfficeWorld,
        now: DateTime<Utc>,
        hidden: bool,
        enabled: bool,
    ) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut notices = Vec::new();
        for (key, team) in &world.teams {
            if world.displayed_phase(key) == TeamPhase::Off {
                continue;
            }
            for employee in &team.employees {
                if employee.departed_at.is_some() || !team.needs_boss(employee, now) {
                    continue;
                }
                let id = (key.clone(), employee.id.clone());
                seen.insert(id.clone());
                let episode = self.episodes.entry(id).or_insert(Episode {
                    since: now,
                    sent: false,
                });
                if hidden
                    && enabled
                    && !episode.sent
                    && now - episode.since >= Duration::seconds(30)
                {
                    episode.sent = true;
                    // 제목·질문·명령 대신 앱이 붙인 부장 이름만 OS에 보낸다.
                    if let Some(boss) = team.employees.first() {
                        notices.push(format!("{} 부장 팀", boss.name));
                    }
                }
            }
        }
        self.episodes.retain(|id, _| seen.contains(id));
        notices.sort();
        notices
    }
}

pub fn show(app: &AppHandle, team_name: &str) {
    if let Err(error) = app
        .notification()
        .builder()
        .title(team_name)
        .body("결재 기다립니다")
        .show()
    {
        // 실패한 에피소드를 매 틱 재시도하면 같은 알림이 연속으로 뜰 수 있다.
        crate::oplog_once!("OS 알림 표시 실패: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{Product, Vendor};
    use crate::office::Team;
    use crate::office::names::NameBook;
    use crate::office::world::Scale;

    fn world(teams: HashMap<TeamKey, Team>) -> OfficeWorld {
        let dir = tempfile::tempdir().unwrap();
        let mut world = OfficeWorld::new(
            Scale::Large,
            NameBook::load(dir.path()),
            chrono::FixedOffset::east_opt(0).unwrap(),
        );
        world.teams = teams;
        world
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::UNIX_EPOCH + Duration::seconds(seconds)
    }

    fn waiting() -> (TeamKey, OfficeWorld) {
        let key = TeamKey {
            host: "windows".into(),
            vendor: Vendor::Claude,
            session: "test".into(),
        };
        let mut team = Team::new("test", Product::ClaudeCli, at(0));
        team.employees[0].name = "김하늘".into();
        team.employees[0].asking = Some("알림에 보내면 안 되는 질문".into());
        (key.clone(), world(HashMap::from([(key, team)])))
    }

    #[test]
    fn hidden_after_thirty_seconds_sends_once_without_question_text() {
        let (_, teams) = waiting();
        let mut notifications = PendingNotifications::default();
        assert!(notifications.poll(&teams, at(0), true, true).is_empty());
        assert!(notifications.poll(&teams, at(29), true, true).is_empty());
        assert_eq!(
            notifications.poll(&teams, at(30), true, true),
            ["김하늘 부장 팀"]
        );
        assert!(notifications.poll(&teams, at(120), true, true).is_empty());
    }

    #[test]
    fn projected_departure_suppresses_notices_and_restarts_wait_on_recovery() {
        let (key, mut teams) = waiting();
        let mut notifications = PendingNotifications::default();
        notifications.poll(&teams, at(0), true, true);
        teams.set_read_failed(&key, true);
        assert!(notifications.poll(&teams, at(30), true, true).is_empty());
        teams.set_read_failed(&key, false);
        assert!(notifications.poll(&teams, at(31), true, true).is_empty());
        assert_eq!(notifications.poll(&teams, at(61), true, true).len(), 1);
        teams.registry_alive(&key, true, at(62));
        teams.registry_alive(&key, false, at(63));
        assert!(notifications.poll(&teams, at(100), true, true).is_empty());
        assert!(teams.teams[&key].employees[0].asking.is_some());
    }

    #[test]
    fn visible_and_disabled_keep_the_same_unsent_episode() {
        let (_, teams) = waiting();
        let mut notifications = PendingNotifications::default();
        assert!(notifications.poll(&teams, at(0), false, false).is_empty());
        assert!(notifications.poll(&teams, at(30), false, true).is_empty());
        assert!(notifications.poll(&teams, at(31), true, false).is_empty());
        assert_eq!(notifications.poll(&teams, at(32), true, true).len(), 1);
        assert!(notifications.poll(&teams, at(33), false, false).is_empty());
        assert!(notifications.poll(&teams, at(34), true, true).is_empty());
    }

    #[test]
    fn rejection_preserves_episode_but_leaving_wait_resets_it() {
        let (key, mut teams) = waiting();
        let mut notifications = PendingNotifications::default();
        notifications.poll(&teams, at(0), true, true);
        assert_eq!(notifications.poll(&teams, at(30), true, true).len(), 1);
        let employee = &mut teams.teams.get_mut(&key).unwrap().employees[0];
        employee.asking = None;
        employee.rejected_at = Some(at(31));
        assert!(notifications.poll(&teams, at(32), true, true).is_empty());
        teams.teams.get_mut(&key).unwrap().employees[0].asking = Some("다시 질문".into());
        assert!(notifications.poll(&teams, at(33), true, true).is_empty());
        let employee = &mut teams.teams.get_mut(&key).unwrap().employees[0];
        employee.asking = None;
        employee.rejected_at = None;
        notifications.poll(&teams, at(34), true, true);
        teams.teams.get_mut(&key).unwrap().employees[0].asking = Some("새 질문".into());
        assert!(notifications.poll(&teams, at(35), true, true).is_empty());
        assert_eq!(notifications.poll(&teams, at(65), true, true).len(), 1);
    }

    #[test]
    fn removing_or_departing_a_team_discards_its_episode() {
        let (key, mut teams) = waiting();
        let mut notifications = PendingNotifications::default();
        notifications.poll(&teams, at(0), true, true);
        teams.teams.get_mut(&key).unwrap().phase = TeamPhase::Off;
        assert!(notifications.poll(&teams, at(30), true, true).is_empty());
        teams.teams.get_mut(&key).unwrap().phase = TeamPhase::Working;
        assert!(notifications.poll(&teams, at(31), true, true).is_empty());
        teams.teams.clear();
        assert!(notifications.poll(&teams, at(61), true, true).is_empty());
    }

    #[test]
    fn replayed_permission_wait_notifies_once_at_thirty_seconds() {
        use crate::office::classify::replay;
        use crate::parse::codex::CodexParser;
        use crate::parse::{Parser, RawRef};
        use std::path::PathBuf;

        let id = "a9e19951-4ff7-996b-7dfa-2cfe04d16f10";
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../fixtures/codex/ask-reject-compact/rollout-2026-03-02T09-00-00-{id}.jsonl"
        ));
        let until: DateTime<Utc> = "2026-03-02T09:00:04.029Z".parse().unwrap();
        let mut parser = CodexParser::new(id);
        let mut events = Vec::new();
        let mut offset = 0;
        for (i, line) in std::fs::read(&path)
            .unwrap()
            .split(|b| *b == b'\n')
            .enumerate()
        {
            let raw = RawRef {
                path: path.clone(),
                generation: 0,
                byte_offset: offset,
                line_no: i as u64 + 1,
            };
            events.extend(parser.parse_line(line, &raw, &mut Vec::new()));
            offset += line.len() as u64 + 1;
        }
        let mut last = DateTime::UNIX_EPOCH;
        events.retain(|event| {
            last = last.max(event.ts.unwrap_or(last));
            last <= until
        });
        let local = chrono::FixedOffset::east_opt(0).unwrap();
        let mut team = replay(id, Product::CodexCli, &[], &events, local);
        team.employees[0].name = "김하늘".into();
        assert!(team.needs_boss(&team.employees[0], until));
        let key = TeamKey {
            host: "windows".into(),
            vendor: Vendor::Codex,
            session: id.into(),
        };
        let mut teams = world(HashMap::from([(key, team)]));
        let mut notifications = PendingNotifications::default();
        for seconds in [0, 29, 30, 60] {
            let now = until + Duration::seconds(seconds);
            for team in teams.teams.values_mut() {
                team.tick(now, local, &mut Vec::new());
            }
            let notices = notifications.poll(&teams, now, true, true);
            assert_eq!(notices.len(), usize::from(seconds == 30));
        }
    }
}
