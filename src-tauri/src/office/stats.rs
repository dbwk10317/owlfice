//! 명령별 평균 소요와 장애 판정 (PLAN 4.7.3)

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};

// ponytail: 평균의 새 값 비중은 정해진 근거가 없다, 실제 사용에서 막대가 튀면 올린다
const EMA_ALPHA: f32 = 0.3;
const SAME_COMMAND_FAILS: u32 = 3;
const BURST_ERRORS: usize = 3;
const BURST_WINDOW: Duration = Duration::seconds(60);

#[derive(Debug, Clone, Default)]
pub struct TeamStats {
    /// command_key → 평균 소요 초
    pub avg: HashMap<String, f32>,
    /// command_norm → 연속 실패 수
    streak: HashMap<String, u32>,
    recent_errors: VecDeque<DateTime<Utc>>,
}

impl TeamStats {
    pub fn record_duration(&mut self, command_key: &str, secs: f32) {
        self.avg
            .entry(command_key.to_string())
            .and_modify(|a| *a += EMA_ALPHA * (secs - *a))
            .or_insert(secs);
    }

    /// Confirmed Err 하나를 센다. Rejected·Unknown·Inferred 는 부르지 않는다. 장애 조건이 서면 true
    pub fn record_error(&mut self, command_norm: Option<&str>, at: DateTime<Utc>) -> bool {
        self.recent_errors.push_back(at);
        while self
            .recent_errors
            .front()
            .is_some_and(|&t| at - t > BURST_WINDOW)
        {
            self.recent_errors.pop_front();
        }
        let same = command_norm.is_some_and(|c| {
            let n = self.streak.entry(c.to_string()).or_insert(0);
            *n += 1;
            *n >= SAME_COMMAND_FAILS
        });
        same || self.recent_errors.len() >= BURST_ERRORS
    }

    /// 성공은 그 명령의 연속 실패를 끊는다
    pub fn record_ok(&mut self, command_norm: Option<&str>) {
        if let Some(c) = command_norm {
            self.streak.remove(c);
        }
    }
}
