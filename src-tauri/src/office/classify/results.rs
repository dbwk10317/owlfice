use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use super::{conflict, home_room, is_review, sentence, verdict};
use crate::office::{
    Activity, BubbleSay, Cue, Incident, Light, Room, Team, TestCounts, TestVerdict,
};
use crate::parse::{Compound, Confidence, EventKind, TestResult, ToolKind, ToolOutcome};

impl Team {
    pub(super) fn on_result(
        &mut self,
        i: usize,
        kind: &EventKind,
        ts: DateTime<Utc>,
        cues: &mut Vec<Cue>,
    ) {
        let EventKind::ToolResult {
            call_id,
            outcome,
            outcome_confidence,
            preview,
            test_result,
        } = kind
        else {
            return;
        };
        let test_result = *test_result;
        let e = &mut self.employees[i];
        // docs/CONTRACTS.md C8 공통 전제
        let (call, was_background) =
            if let Some(k) = e.pending.iter().position(|c| &c.call_id == call_id) {
                (e.pending.remove(k), false)
            } else if let Some(k) = e.background.iter().position(|c| &c.call_id == call_id) {
                (e.background.remove(k), true)
            } else {
                return;
            };
        let agent = e.id.clone();
        if was_background {
            cues.push(Cue::Paper {
                agent: agent.clone(),
                on: false,
            });
        }
        if matches!(call.tool, ToolKind::Ask | ToolKind::ExitPlanMode) {
            e.asking = e.pending.iter().rev().find_map(|c| c.question.clone());
        }
        if *outcome != ToolOutcome::Started {
            e.quiet_since = Some(ts);
            // 턴이 끝난 뒤 온 백그라운드 완료도 다음 응답을 부른다 (docs/CONTRACTS.md C2 응답 대기)
            e.waiting_user_since = None;
        }
        let home = home_room(e);
        let confirmed = *outcome_confidence == Confidence::Confirmed;
        if confirmed
            && ((call.tool == ToolKind::EnterPlanMode
                && matches!(outcome, ToolOutcome::Err | ToolOutcome::Rejected))
                || (call.tool == ToolKind::ExitPlanMode
                    && matches!(outcome, ToolOutcome::Ok | ToolOutcome::Rejected)))
        {
            e.planning = false;
        }
        let declares = call.summary.compound != Compound::Mixed;
        let test_verdict = if call.tool == ToolKind::Test && confirmed && declares {
            match outcome {
                ToolOutcome::Ok => Some(verdict(test_result.map_or(0, |t| t.failed))),
                ToolOutcome::Err
                    if call.summary.compound == Compound::Single
                        || test_result.is_some_and(|t| t.failed > 0) =>
                {
                    Some(TestVerdict::Fail)
                }
                _ => None,
            }
        } else {
            None
        };
        let check_verdict = if call.tool == ToolKind::Build && confirmed && declares {
            match outcome {
                ToolOutcome::Ok => Some(TestVerdict::Pass),
                ToolOutcome::Err if call.summary.compound == Compound::Single => {
                    Some(TestVerdict::Fail)
                }
                _ => None,
            }
        } else {
            test_verdict
        };
        if let Some(verdict) = check_verdict {
            e.last_check_failed = verdict == TestVerdict::Fail;
        }
        if call.tool == ToolKind::Test && *outcome != ToolOutcome::Started {
            if let Some(verdict) = test_verdict {
                let counts = &mut self
                    .daily
                    .entry(ts.with_timezone(&self.local).date_naive())
                    .or_default()
                    .0;
                match verdict {
                    TestVerdict::Pass => counts.passed = counts.passed.saturating_add(1),
                    TestVerdict::Fail => counts.failed = counts.failed.saturating_add(1),
                }
                let delta = match verdict {
                    TestVerdict::Pass => TestCounts {
                        passed: 1,
                        failed: 0,
                    },
                    TestVerdict::Fail => TestCounts {
                        passed: 0,
                        failed: 1,
                    },
                };
                Arc::make_mut(&mut self.daily_events).push((ts, delta, 0));
            }
            if test_verdict == Some(TestVerdict::Pass) {
                e.pass_streak = e.pass_streak.saturating_add(1);
                if e.pass_streak >= 3 {
                    e.medal_until = Some(ts + Duration::minutes(5));
                }
            } else {
                e.pass_streak = 0;
            }
        }
        let own = match outcome {
            ToolOutcome::Rejected => Some(conflict::REJECTED),
            ToolOutcome::Err if confirmed => Some(conflict::ACCIDENT),
            _ => None,
        };
        match outcome {
            ToolOutcome::Started => {
                e.background.push(call.clone());
                cues.push(Cue::Paper { agent, on: true });
                let n = e.background.len();
                e.log_activity(Activity::new(
                    sentence::background(&call, n),
                    home,
                    ts,
                    Confidence::Confirmed,
                ));
            }
            ToolOutcome::Ok if confirmed => {
                // 백그라운드 소요는 실행 시간이 아니라 완료 알림을 읽은 때까지라 평균에 넣지 않는다
                if !was_background && let Some(key) = &call.summary.command_key {
                    let secs = (ts - call.started).num_milliseconds() as f32 / 1000.0;
                    self.stats.record_duration(key, secs);
                }
                self.stats.record_ok(call.summary.command_norm.as_deref());
                let e = &mut self.employees[i];
                match (&call.tool, declares) {
                    (ToolKind::Test, true) => {
                        self.props.light = if test_verdict == Some(TestVerdict::Fail) {
                            Light::Red
                        } else {
                            Light::Green
                        };
                        self.props.light_at = ts;
                        let t = test_result.unwrap_or(TestResult {
                            passed: 0,
                            failed: 0,
                        });
                        self.props.test = Some(verdict(t.failed));
                        self.props.test_at = ts;
                        cues.push(Cue::Stamp {
                            confirmed_pass: test_verdict == Some(TestVerdict::Pass),
                            agent: agent.clone(),
                            at: ts,
                            passed: t.passed,
                            failed: t.failed,
                            estimated: false,
                        });
                    }
                    (ToolKind::Test, false) => {
                        if let Some(t) = test_result {
                            self.props.test = Some(verdict(t.failed));
                            self.props.test_at = ts;
                            cues.push(Cue::Stamp {
                                confirmed_pass: test_verdict == Some(TestVerdict::Pass),
                                agent: agent.clone(),
                                at: ts,
                                passed: t.passed,
                                failed: t.failed,
                                estimated: true,
                            });
                        }
                    }
                    (ToolKind::Build, true) => {
                        self.props.light = Light::Green;
                        self.props.light_at = ts;
                    }
                    (ToolKind::GitCommit, true) => {
                        self.commits = self.commits.saturating_add(1);
                        let commits = &mut self
                            .daily
                            .entry(ts.with_timezone(&self.local).date_naive())
                            .or_default()
                            .1;
                        *commits = commits.saturating_add(1);
                        Arc::make_mut(&mut self.daily_events).push((ts, TestCounts::default(), 1));
                    }
                    (ToolKind::GitPush | ToolKind::PrCreate, true) => cues.push(Cue::Truck),
                    (ToolKind::Release, true) => cues.push(Cue::Fireworks),
                    _ => {}
                }
                // 테스트 전부 통과는 빨간불을 끄고 장애를 푼다 (docs/CONTRACTS.md C8). Mixed 는 종료 코드가
                // 통과를 보증하지 못해 본문의 숫자로만 본다
                let all_pass = call.tool == ToolKind::Test
                    && if declares {
                        test_result.is_none_or(|t| t.failed == 0)
                    } else {
                        test_result.is_some_and(|t| t.failed == 0)
                    };
                if all_pass {
                    e.red = None;
                }
                // 리뷰 도구는 끝나도 리뷰실에 남아 이어지는 지적을 센다
                let room = if is_review(&call.tool) {
                    Room::Review
                } else {
                    home
                };
                e.log_activity(Activity::new(
                    sentence::done(&call, declares, test_result, e.review_pens),
                    room,
                    ts,
                    Confidence::Confirmed,
                ));
                if all_pass {
                    e.say(ts, BubbleSay::AllGreen);
                    self.end_incident(ts, cues);
                }
            }
            ToolOutcome::Err if confirmed => {
                e.counters.errors += 1;
                cues.push(Cue::Shake {
                    agent: agent.clone(),
                });
                self.props.trash += 1;
                self.props.safe_since = ts;
                match &call.tool {
                    ToolKind::Build => {
                        if check_verdict == Some(TestVerdict::Fail) {
                            self.props.light = Light::Red;
                            self.props.light_at = ts;
                        }
                    }
                    ToolKind::Test => {
                        if check_verdict == Some(TestVerdict::Fail) {
                            self.props.light = Light::Red;
                            self.props.light_at = ts;
                            let fails = e.red.as_ref().map_or(0, |r| r.1);
                            e.red = Some((sentence::failed_test(&call, preview), fails + 1));
                            self.props.test = Some(TestVerdict::Fail);
                            self.props.test_at = ts;
                        }
                        if let Some(t) = test_result {
                            cues.push(Cue::Stamp {
                                confirmed_pass: test_verdict == Some(TestVerdict::Pass),
                                agent: agent.clone(),
                                at: ts,
                                passed: t.passed,
                                failed: t.failed,
                                estimated: !declares,
                            });
                        }
                    }
                    ToolKind::GitPush | ToolKind::PrCreate => cues.push(Cue::Return),
                    _ => {}
                }
                if matches!(call.tool, ToolKind::Build | ToolKind::Test) && check_verdict.is_none()
                {
                    self.restore_light(&call);
                }
                if self
                    .stats
                    .record_error(call.summary.command_norm.as_deref(), ts)
                {
                    match &mut self.incident {
                        Some(inc) => inc.last_error = ts,
                        None => {
                            self.incident = Some(Incident {
                                since: ts,
                                last_error: ts,
                            });
                            cues.push(Cue::Siren { on: true });
                        }
                    }
                } else if let Some(inc) = &mut self.incident {
                    inc.last_error = ts;
                }
                // 사고 1건은 말풍선이 없다 (HANDOFF 5절). 장애는 상황실 문구가 맡는다
                self.employees[i].log_activity(Activity {
                    bad: true,
                    ..Activity::new(
                        sentence::accident(&call, preview),
                        home,
                        ts,
                        Confidence::Confirmed,
                    )
                });
            }
            ToolOutcome::Rejected => {
                e.rejected_at = Some(ts);
                cues.push(Cue::Reject { agent });
                e.log_activity(Activity {
                    bad: confirmed,
                    ..Activity::new(sentence::rejected(&call), home, ts, Confidence::Confirmed)
                });
                self.restore_light(&call);
            }
            // Unknown, 또는 본문으로 짐작한 Ok/Err. 신호등·무재해·출고·평균을 건드리지 않는다 (S59)
            _ => {
                e.log_activity(Activity::new(
                    sentence::unknown(&call),
                    home,
                    ts,
                    Confidence::Inferred,
                ));
                e.say(ts, BubbleSay::UnknownResult);
                self.restore_light(&call);
            }
        }
        // 남은 호출이 결과 연출을 덮는다
        if self.refresh(i, ts)
            && let Some(m) = own
        {
            self.conflict(i, ts, m, cues);
        }
    }
}
