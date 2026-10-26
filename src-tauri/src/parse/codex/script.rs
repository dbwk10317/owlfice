//! exec 스크립트의 리터럴과 호출 구문만 추출한다.

#[derive(Default)]
pub(super) struct ExecScan {
    /// `cmd:` 문자열 리터럴. 변수·`${}` 템플릿은 못 뽑는다
    pub(super) cmds: Vec<String>,
    /// `tools.<이름>` 으로 부른 도구
    pub(super) tools: Vec<String>,
    /// 첫 `tools.apply_patch(` 의 리터럴 인자
    pub(super) patch: Option<String>,
    pub(super) question: Option<String>,
    /// `sandbox_permissions: "require_escalated"` 리터럴
    pub(super) escalated: bool,
    /// `write_stdin` 의 (session_id, chars)
    pub(super) stdin: Vec<(Option<String>, Option<String>)>,
}

#[derive(Debug, PartialEq)]
enum Tok {
    /// 식별자와 숫자
    Word(String),
    /// `plain` 은 끝까지 닫혔고 `${}` 가 없는 리터럴
    Str {
        text: String,
        plain: bool,
    },
    Punct(char),
}

pub(super) fn scan_exec(src: &str) -> ExecScan {
    let t = tokenize(src);
    let mut s = ExecScan::default();
    let word = |i: usize, w: &str| matches!(t.get(i), Some(Tok::Word(x)) if x == w);
    let punct = |i: usize, c: char| t.get(i) == Some(&Tok::Punct(c));
    // `key: 값` 의 값. 키는 식별자거나 문자열이다
    let value_of = |i: usize, key: &str| -> Option<&Tok> {
        let is_key = match t.get(i)? {
            Tok::Word(w) => w == key,
            Tok::Str { text, .. } => text == key,
            Tok::Punct(_) => false,
        };
        if is_key && punct(i + 1, ':') {
            t.get(i + 2)
        } else {
            None
        }
    };
    for i in 0..t.len() {
        if word(i, "tools")
            && punct(i + 1, '.')
            && let Some(Tok::Word(name)) = t.get(i + 2)
        {
            s.tools.push(name.clone());
            if name == "apply_patch"
                && s.patch.is_none()
                && punct(i + 3, '(')
                && let Some(Tok::Str { text, .. }) = t.get(i + 4)
            {
                s.patch = Some(text.clone());
            }
            if name == "write_stdin" {
                let (mut sid, mut chars) = (None, None);
                // 이 호출의 짝 괄호 안만 본다. 뒤의 무관한 객체가 이기면 안 된다
                let (mut j, mut depth) = (i + 3, 0);
                while j < t.len() {
                    if punct(j, '(') {
                        depth += 1;
                    } else if punct(j, ')') {
                        depth -= 1;
                        if depth <= 0 {
                            break;
                        }
                    } else if depth == 0 {
                        break;
                    }
                    match value_of(j, "session_id") {
                        Some(Tok::Word(n)) => sid = Some(n.clone()),
                        Some(Tok::Str { text, .. }) => sid = Some(text.clone()),
                        _ => {}
                    }
                    if let Some(Tok::Str { text, .. }) = value_of(j, "chars") {
                        chars = Some(text.clone());
                    }
                    j += 1;
                }
                s.stdin.push((sid, chars));
            }
        }
        // 객체 리터럴의 키 자리만 본다. 삼항식의 `"cmd"` 나 `cmd: "git " + sub` 는 값이 아니다
        if let Some(Tok::Str { text, plain: true }) = value_of(i, "cmd")
            && i > 0
            && matches!(t[i - 1], Tok::Punct('{' | ','))
            && matches!(t.get(i + 3), Some(Tok::Punct(',' | '}' | ')')))
        {
            s.cmds.push(text.clone());
        }
        if let Some(Tok::Str { text, .. }) = value_of(i, "sandbox_permissions")
            && text == "require_escalated"
        {
            s.escalated = true;
        }
        if s.question.is_none()
            && let Some(Tok::Str { text, .. }) = value_of(i, "question")
        {
            s.question = Some(text.clone());
        }
    }
    s
}

/// 문자열·주석·정규식 리터럴을 건너뛰는 정도의 JS 토큰 나누기
fn tokenize(src: &str) -> Vec<Tok> {
    let cs: Vec<char> = src.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let next = cs.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && next == Some('/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i < cs.len() && !(cs[i] == '*' && cs.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if c == '/'
            && !matches!(
                out.last(),
                Some(Tok::Word(_) | Tok::Str { .. } | Tok::Punct(')' | ']'))
            )
            && let Some(end) = regex_end(&cs, i)
        {
            // 안의 따옴표가 문자열로 읽히면 뒤의 `cmd:` 를 삼킨다
            out.push(Tok::Str {
                text: String::new(),
                plain: false,
            });
            i = end;
        } else if matches!(c, '"' | '\'' | '`') {
            let (text, plain, end) = read_str(&cs, i);
            out.push(Tok::Str { text, plain });
            i = end;
        } else if is_word(c) {
            let start = i;
            while i < cs.len() && is_word(cs[i]) {
                i += 1;
            }
            out.push(Tok::Word(cs[start..i].iter().collect()));
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

/// `start` 의 `/` 가 여는 정규식의 다음 위치. 한 줄 안에서 안 닫히면 정규식이 아니다
fn regex_end(cs: &[char], start: usize) -> Option<usize> {
    let (mut i, mut class) = (start + 1, false);
    while i < cs.len() {
        match cs[i] {
            '\n' => return None,
            '\\' => i += 1,
            '[' => class = true,
            ']' => class = false,
            '/' if !class => return Some(i + 1),
            _ => {}
        }
        i += 1;
    }
    None
}

fn read_str(cs: &[char], start: usize) -> (String, bool, usize) {
    let q = cs[start];
    let mut s = String::new();
    let mut plain = true;
    let mut i = start + 1;
    while i < cs.len() {
        let c = cs[i];
        if c == q {
            return (s, plain, i + 1);
        }
        if c == '\\' {
            let Some(&e) = cs.get(i + 1) else {
                break;
            };
            i += 2;
            match e {
                'n' => s.push('\n'),
                't' => s.push('\t'),
                'r' => s.push('\r'),
                'b' => s.push('\u{8}'),
                'f' => s.push('\u{c}'),
                'v' => s.push('\u{b}'),
                '0' => s.push('\0'),
                'x' => i = push_hex(cs, i, 2, &mut s),
                'u' if cs.get(i) == Some(&'{') => match cs[i..].iter().position(|&c| c == '}') {
                    Some(len) => {
                        let hex: String = cs[i + 1..i + len].iter().collect();
                        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            Some(ch) => s.push(ch),
                            None => plain = false,
                        }
                        i += len + 1;
                    }
                    None => plain = false,
                },
                'u' => i = push_hex(cs, i, 4, &mut s),
                // 줄 잇기
                '\n' => {}
                '\r' => {
                    if cs.get(i) == Some(&'\n') {
                        i += 1;
                    }
                }
                other => s.push(other),
            }
            continue;
        }
        if q == '`' && c == '$' && cs.get(i + 1) == Some(&'{') {
            plain = false;
        }
        // 따옴표 문자열 안의 맨 개행은 문법 오류다. 닫히지 않은 것으로 보고 거기서 끊는다
        if q != '`' && c == '\n' {
            return (s, false, i);
        }
        s.push(c);
        i += 1;
    }
    (s, false, i)
}

fn hex_at(cs: &[char], i: usize, n: usize) -> Option<u32> {
    let digits = cs.get(i..i + n)?;
    digits
        .iter()
        .all(char::is_ascii_hexdigit)
        .then(|| u32::from_str_radix(&digits.iter().collect::<String>(), 16).ok())?
}

/// `\x`·`\u` 뒤 16진 `n` 자리. 모양이 틀리면 글자를 그대로 둔다
fn push_hex(cs: &[char], i: usize, n: usize, s: &mut String) -> usize {
    let code = hex_at(cs, i, n);
    // 상위 서로게이트 뒤에 `\u` 하위가 오면 한 글자다
    if let Some(hi) = code.filter(|c| n == 4 && (0xD800..0xDC00).contains(c)) {
        let lo = (cs.get(i + 4) == Some(&'\\') && cs.get(i + 5) == Some(&'u'))
            .then(|| hex_at(cs, i + 6, 4))
            .flatten()
            .filter(|lo| (0xDC00..0xE000).contains(lo));
        if let Some(ch) =
            lo.and_then(|lo| char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)))
        {
            s.push(ch);
            return i + 10;
        }
    }
    match code.and_then(char::from_u32) {
        Some(ch) => {
            s.push(ch);
            i + n
        }
        None => {
            s.push(if n == 2 { 'x' } else { 'u' });
            i
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scan_exec;

    #[test]
    fn write_stdin_args_stop_at_the_closing_paren() {
        let s = scan_exec(
            "await tools.write_stdin({session_id:4242}); const o = {session_id: 9, chars: \"\\u0003\"};",
        );
        assert_eq!(s.stdin, [(Some("4242".into()), None)]);
    }
}
