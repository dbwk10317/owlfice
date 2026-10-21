use super::Rank;

/// 서브에이전트 직급 (docs/CONTRACTS.md C8). 부분 문자열이 아니라 토큰 단위로 맞춘다.
/// `gemini` 안의 `mini` 가 Sawon으로 걸리면 안 되기 때문이다.
pub fn rank_of(model: Option<&str>, effort: Option<&str>) -> Rank {
    let lower = model.unwrap_or("").to_lowercase();
    let tokens: Vec<&str> = lower
        .split(['-', '.', '_'])
        .filter(|t| !t.is_empty())
        .collect();
    let has = |t: &str| tokens.contains(&t);
    // 연속 토큰 조합 (`gpt-5` 는 [gpt, 5])
    let seq = |s: &[&str]| tokens.windows(s.len()).any(|w| w == s);

    if tokens.is_empty() || ["haiku", "mini", "nano", "lite"].into_iter().any(has) {
        return Rank::Sawon;
    }
    let high = effort.is_some_and(|e| ["high", "xhigh"].iter().any(|h| e.eq_ignore_ascii_case(h)));
    if ["opus", "fable", "mythos", "o3", "argon", "sol", "astra"]
        .into_iter()
        .any(has)
        || (has("gemini") && has("pro"))
        || (seq(&["gpt", "5"]) && high)
    {
        return Rank::Gwajang;
    }
    // 표의 3번(sonnet, gpt-5, gpt-4.1, gemini flash)과 4번(그 외)은 둘 다 대리다.
    Rank::Daeri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_examples() {
        let cases = [
            ("gemini-2.5-flash-lite", None, Rank::Sawon),
            ("gemini-2.5-pro", None, Rank::Gwajang),
            ("gemini-2.5-flash", None, Rank::Daeri),
            ("gpt-5-mini", None, Rank::Sawon),
            ("o3-mini", None, Rank::Sawon),
            ("gpt-5", Some("high"), Rank::Gwajang),
            ("gpt-5", Some("xhigh"), Rank::Gwajang),
            ("gpt-5", None, Rank::Daeri),
            ("gpt-4.1", None, Rank::Daeri),
            ("claude-opus-5-5", None, Rank::Gwajang),
            ("gpt-6.1-sol", None, Rank::Gwajang),
            ("gpt-6-astra", None, Rank::Gwajang),
            ("gemini-4-argon-trial", None, Rank::Gwajang),
            ("gpt-6.1-sol-mini", None, Rank::Sawon),
        ];
        for (model, effort, want) in cases {
            assert_eq!(rank_of(Some(model), effort), want, "{model} {effort:?}");
        }
    }

    #[test]
    fn missing_model_is_staff() {
        assert_eq!(rank_of(None, None), Rank::Sawon);
    }

    #[test]
    fn case_and_unknown_model() {
        assert_eq!(rank_of(Some("Claude-OPUS-4"), None), Rank::Gwajang);
        assert_eq!(rank_of(Some("gpt-5.5-turbo"), None), Rank::Daeri);
    }
}
