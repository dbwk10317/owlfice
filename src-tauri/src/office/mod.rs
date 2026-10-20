pub mod names;
pub mod rank;

/// 직원 직급 (PLAN 4.7.4). 메인 에이전트가 부장인 것은 부르는 쪽이 정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Bujang,
    Gwajang,
    Daeri,
    Sawon,
}
