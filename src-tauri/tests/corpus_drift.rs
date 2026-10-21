//! 코퍼스 drift 테스트 (PLAN 4.12). `OWLFICE_CORPUS_DIR` 아래 실제 세션 파일을 제품 파서로 끝까지 읽어
//! 포맷이 바뀌었는지 본다. 출력은 이름·개수·경로까지만이고 레코드 내용은 찍지 않는다 (AGENTS 8절)

use std::collections::BTreeMap;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Instant;

use owlfice_lib::discover::{ReadMode, Site, Vendor, windows_host};
use owlfice_lib::parse::{RawRef, Unhandled};
use owlfice_lib::tail::scheduler::{list_site, make_parser};
use owlfice_lib::tail::{MAX_READ_BYTES, TailReader};

#[derive(Default)]
struct Counts {
    /// 미대응 (제품, 이름) 별 개수
    unhandled: BTreeMap<(&'static str, String), u64>,
    /// JSON 객체가 아닌 줄 수 (제품별)
    bad_lines: BTreeMap<&'static str, u64>,
}

#[test]
fn corpus_drift() {
    let Some(dir) = std::env::var_os("OWLFICE_CORPUS_DIR") else {
        println!("OWLFICE_CORPUS_DIR 없음: 코퍼스 drift 테스트를 건너뜁니다");
        return;
    };
    let dir = PathBuf::from(dir);
    let started = Instant::now();
    let roots = [
        (Vendor::Claude, dir.join(".claude/projects")),
        (Vendor::Codex, dir.join(".codex/sessions")),
    ];

    let mut files = 0u64;
    let mut failed: Vec<String> = Vec::new();
    let mut counts = Counts::default();
    for (vendor, root) in roots {
        let site = Site {
            host: windows_host(),
            vendor,
            root,
            read_mode: ReadMode::Append,
        };
        let listing = match list_site(&site, &mut 0) {
            Ok(l) => l,
            Err(_) if !site.root.is_dir() => {
                println!("{}: 세션 폴더 없음", vendor.label());
                continue;
            }
            Err(e) => {
                failed.push(format!("{} 나열 실패: {e}", site.root.display()));
                continue;
            }
        };
        for (path, _) in &listing.files {
            files += 1;
            // 파서가 패닉해도 다음 파일을 계속 본다
            match catch_unwind(AssertUnwindSafe(|| read_file(path, vendor, &mut counts))) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failed.push(format!("{}: {e}", path.display())),
                Err(_) => failed.push(format!("{}: 패닉", path.display())),
            }
        }
    }

    println!("파일 {files}개, {:.1}초", started.elapsed().as_secs_f64());
    for ((product, name), n) in &counts.unhandled {
        println!("미대응 {product} {name}: {n}");
    }
    for (product, n) in &counts.bad_lines {
        println!("JSON 객체 아닌 줄 {product}: {n}");
    }
    for f in &failed {
        println!("실패 {f}");
    }
    assert!(
        files > 0,
        "가리킨 폴더에서 세션 파일을 찾지 못했다: {}",
        dir.display()
    );
    assert!(
        failed.is_empty() && counts.unhandled.is_empty() && counts.bad_lines.is_empty(),
        "실패 {}건, 미대응 {}종, JSON 객체 아닌 줄 {}개",
        failed.len(),
        counts.unhandled.len(),
        counts.bad_lines.values().sum::<u64>()
    );
}

/// 앱이 읽는 방식(TailReader 줄 단위)으로 끝까지 읽어 미대응 record_type·system_subtype 을 센다
fn read_file(path: &Path, vendor: Vendor, counts: &mut Counts) -> io::Result<()> {
    let product = vendor.label();
    let mut parser = make_parser(path, vendor);
    let mut reader = TailReader::open(path)?;
    let mut prev_end = None;
    let mut bad_in_file = false;
    loop {
        let pending = reader.read_new_lines(MAX_READ_BYTES)?;
        let end = pending.finished(&mut prev_end);
        for line in &pending.lines {
            let raw = RawRef {
                path: path.to_path_buf(),
                generation: u64::from(reader.generation()),
                byte_offset: line.offset,
                line_no: line.line_no,
            };
            // TailReader 는 개행으로 끝난 줄만 주므로 쓰는 중인 줄은 여기 걸리지 않는다
            if !serde_json::from_slice::<serde_json::Value>(&line.bytes)
                .is_ok_and(|v| v.is_object())
            {
                *counts.bad_lines.entry(product).or_default() += 1;
                bad_in_file = true;
            }
            let mut unhandled = Vec::new();
            parser.parse_line(&line.bytes, &raw, &mut unhandled);
            for u in unhandled {
                let name = match u {
                    Unhandled::RecordType { type_name, .. } => format!("record_type {type_name}"),
                    Unhandled::SystemSubtype { subtype } => format!("system_subtype {subtype}"),
                    _ => continue,
                };
                *counts.unhandled.entry((product, name)).or_default() += 1;
            }
        }
        reader.commit(pending);
        if end {
            return if bad_in_file {
                Err(io::Error::other("JSON 객체가 아닌 줄이 있음"))
            } else {
                Ok(())
            };
        }
    }
}
