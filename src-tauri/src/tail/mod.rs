//! 증분 파일 읽기 (docs/CONTRACTS.md C6)

pub mod scheduler;

use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::Hasher;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::time::SystemTime;

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};

/// 한 번의 `read_new_lines` 가 읽는 상한 (docs/CONTRACTS.md C6). 호출자가 `max_bytes` 로 넘긴다
pub const MAX_READ_BYTES: usize = 8 * 1024 * 1024;
/// 개행 없는 미완성 줄을 쥐고 있을 수 있는 상한. 넘으면 그 줄을 버린다
const MAX_REMAINDER: usize = 64 * 1024 * 1024;
/// offset 직전 몇 바이트를 해시로 대조하는가
pub(crate) const TAIL_HASH_BYTES: usize = 4096;

/// (볼륨 일련번호, 파일 인덱스)
type FileId = (u32, u64);

/// 한 줄. `offset` 은 파일 안 시작 바이트(0 기준), `line_no` 는 1 기준.
/// 끝의 `\n` 과 그 앞의 `\r` 은 뺀 바이트다. UTF-8 해석은 파서가 한다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub offset: u64,
    pub line_no: u64,
    pub bytes: Vec<u8>,
}

/// `read_new_lines` 의 결과. `commit` 하기 전에는 TailReader의 상태를 바꾸지 않는다
#[derive(Debug)]
pub struct Pending {
    /// true면 `lines` 는 새 세대의 0바이트부터다. 상위는 docs/CONTRACTS.md C4 재구성 규칙대로 다시 만든다
    pub reset: bool,
    pub lines: Vec<Line>,
    /// 64MB를 넘어 버린 줄 수
    pub dropped_lines: u32,
    generation: u32,
    offset: u64,
    remainder: Vec<u8>,
    tail_hash: u64,
    line_no: u64,
    skipping: bool,
    len: u64,
    mtime: SystemTime,
    /// 경로 대조로 준비해 둔 새 핸들로 갈아끼운다
    swap_handle: bool,
    file_id: Option<FileId>,
}

impl Pending {
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// 끝까지 읽는 반복을 멈출 때인가. 줄이 없어도 끝이 아닐 수 있다 (8MB 넘는 한 줄, 버리는 64MB 줄).
    /// 읽은 끝이 파일 길이에 닿았거나, Reset 이 아닌데 직전보다 나아가지 못했으면 멈춘다
    /// (WSL 9P 처럼 stat 길이보다 적게 읽히면 같은 읽기가 되풀이된다). `prev_end` 는 부르는 쪽이 들고 다닌다
    pub fn finished(&self, prev_end: &mut Option<u64>) -> bool {
        let stalled = prev_end.is_some_and(|p| self.stalled_after(p));
        *prev_end = Some(self.end());
        self.end() >= self.len || stalled
    }

    /// Reset 이 아닌데 읽은 끝이 `prev_end` 보다 나아가지 못했나
    fn stalled_after(&self, prev_end: u64) -> bool {
        !self.reset && self.end() <= prev_end
    }

    fn end(&self) -> u64 {
        self.offset + self.remainder.len() as u64
    }
}

pub struct TailReader {
    path: PathBuf,
    file: File,
    /// 경로 대조에서 다른 파일로 바뀐 것을 본 새 핸들. commit 전까지 `file` 은 그대로 둔다
    next: Option<File>,
    /// 마지막 개행 직후까지 소비한 바이트. remainder는 포함하지 않는다
    offset: u64,
    /// offset 이후 개행 없이 읽어 둔 바이트
    remainder: Vec<u8>,
    /// 64MB를 넘은 줄을 버리는 중이다. 다음 개행까지 읽는 대로 버린다
    skipping: bool,
    /// 지금까지 소비한 줄 수 (다음 줄 번호 - 1)
    line_no: u64,
    last_len: u64,
    last_mtime: SystemTime,
    file_id: Option<FileId>,
    /// offset 직전 4KB의 해시 (길이가 늘었을 때의 덮어쓰기 보조 감지, 규칙 (d))
    tail_hash: u64,
    /// 파일 세대. Reset마다 +1. (`gen` 은 2024 edition 예약어라 이 이름을 쓴다)
    generation: u32,
    /// 마지막 commit 이 읽은 끝을 앞으로 옮기지 못했다
    stalled: bool,
}

pub(crate) fn hash_bytes(b: &[u8]) -> u64 {
    // 메모리 안에서만 비교하므로 버전 간 안정성은 필요 없다
    let mut h = DefaultHasher::new();
    h.write(b);
    h.finish()
}

fn file_id(f: &File) -> Option<FileId> {
    // SAFETY: 구조체는 0으로 채워 넘기고, 핸들은 f가 살아 있는 동안 유효하다
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) };
    (ok != 0).then(|| {
        (
            info.dwVolumeSerialNumber,
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        )
    })
}

pub(crate) fn read_range(mut f: &File, start: u64, n: u64) -> io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(n).read_to_end(&mut buf)?;
    Ok(buf)
}

impl TailReader {
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        // std의 Windows 기본 공유 모드는 READ|WRITE|DELETE라 다른 프로세스가 쓰는 중에도 읽힌다
        let file = File::open(&path)?;
        let last_mtime = file.metadata()?.modified()?;
        Ok(Self {
            file_id: file_id(&file),
            path,
            file,
            next: None,
            offset: 0,
            remainder: Vec::new(),
            skipping: false,
            line_no: 0,
            last_len: 0,
            last_mtime,
            tail_hash: hash_bytes(&[]),
            generation: 0,
            stalled: false,
        })
    }

    /// 세대를 이어 연다. 재구성으로 다시 연 파일의 RawRef 세대가 0 으로 돌아가 옛 커서와 겹치지 않게
    pub fn open_at(path: impl Into<PathBuf>, generation: u32) -> io::Result<Self> {
        Ok(Self {
            generation,
            ..Self::open(path)?
        })
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub(crate) fn file(&self) -> &File {
        &self.file
    }

    /// commit 한 위치 뒤에 개행 없이 남아 읽지 못한 바이트 수. 버리는 중인 줄은 세지 않는다
    pub fn unread_len(&self) -> usize {
        self.remainder.len()
    }

    /// 지금 더 읽을 것이 없나. `len` 은 부르는 쪽이 방금 경로로 본 길이다. 마지막 읽기 뒤 길이가 바뀌었으면 거짓이다.
    /// 마지막 읽기가 본 파일 끝까지 소비했으면 참이다 (개행 없는 마지막 조각은 읽어 둔 것으로 센다).
    /// 덜 읽었어도 그 읽기가 나아가지 못했으면 참이다. WSL 9P 처럼 stat 길이가 읽히는 바이트보다 크면 끝까지 갈 수 없다
    pub fn caught_up(&self, len: u64) -> bool {
        // ponytail: 나아가지 못한 읽기 한 번이면 다 읽음으로 본다(9P 짧은 읽기가 일시적이면 덜 읽은 채 닫는다), 실측에서 일시적 짧은 읽기가 보이면 연속 몇 번으로 올린다
        len == self.last_len && (self.consumed() >= self.last_len || self.stalled)
    }

    fn consumed(&self) -> u64 {
        self.offset + self.remainder.len() as u64
    }

    /// stat만 한다. 길이·mtime이 바뀌었거나, 아직 안 읽은 바이트가 남아 있으면 true.
    /// Codex 앱은 mtime 없이 길이만 늘리므로 길이 변화만으로도 true다
    pub fn changed(&mut self) -> io::Result<bool> {
        let m = self.file.metadata()?;
        Ok(self.next.is_some()
            || m.len() != self.last_len
            || m.modified()? != self.last_mtime
            || m.len() > self.consumed())
    }

    /// Scan 시점에 경로의 파일 ID와 열린 핸들의 ID를 대조한다 (docs/CONTRACTS.md C5).
    /// 다르면 새 핸들을 따로 쥐어 두고, 다음 `read_new_lines` 가 Reset으로 알린다.
    /// 상태는 commit 때만 바뀌므로 여기서도 `file` 을 바꾸지 않는다
    pub fn verify_path(&mut self) -> io::Result<()> {
        let f = File::open(&self.path)?;
        self.next = match (file_id(&f), self.file_id) {
            (Some(a), Some(b)) if a != b => Some(f),
            _ => None,
        };
        Ok(())
    }

    /// offset + remainder.len() 부터 최대 max_bytes 읽어 완전한 줄만 돌려준다.
    /// offset·remainder는 아직 움직이지 않는다 (2단계 commit)
    pub fn read_new_lines(&mut self, max_bytes: usize) -> io::Result<Pending> {
        let swap_handle = self.next.is_some();
        let file = self.next.as_ref().unwrap_or(&self.file);
        let meta = file.metadata()?;
        let (len, mtime) = (meta.len(), meta.modified()?);
        let consumed = self.consumed();
        let max = max_bytes as u64;

        let mut reset = swap_handle
            || len < consumed // (a)
            || (len == consumed && mtime != self.last_mtime); // (c)
        let mut window = Vec::new();
        let mut w = 0; // window 앞쪽 해시 구간 길이
        if !reset && len > consumed {
            // (d) remainder 구간과 offset 직전 4KB를 다시 읽어 캐시와 비교하고, 같은 읽기로 새 바이트도 받는다
            w = self.offset.min(TAIL_HASH_BYTES as u64) as usize;
            let start = self.offset - w as u64;
            window = read_range(file, start, (consumed - start) + (len - consumed).min(max))?;
            let rem_end = w + self.remainder.len();
            if window.len() < rem_end {
                // 읽는 사이에 줄었다. 상태를 안 건드렸으니 다음 틱에 (a)가 잡는다
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            // ponytail: offset 앞 4KB보다 앞쪽만 바뀐 파일은 못 잡는다, 앞부분 재작성이 관찰되면 Scan에서 전체 해시 비교로 올린다
            reset = hash_bytes(&window[..w]) != self.tail_hash
                || window[w..rem_end] != self.remainder[..];
        }

        let unchanged = !reset && len == consumed; // 새 바이트가 없으면 window도 비어 tail을 못 만든다
        let (base, mut skipping, mut line_no, generation, prev, mut data);
        if reset {
            window = read_range(file, 0, len.min(max))?;
            (base, skipping, line_no, generation) = (0, false, 0, self.generation + 1);
            (prev, data) = (&[][..], window);
        } else {
            (base, skipping, line_no, generation) =
                (self.offset, self.skipping, self.line_no, self.generation);
            let rem_end = w + self.remainder.len();
            prev = &window[..w];
            data = self.remainder.clone();
            data.extend_from_slice(&window[rem_end.min(window.len())..]);
        }

        let mut lines = Vec::new();
        let mut dropped_lines = 0;
        let mut pos = 0;
        if skipping {
            match data.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    pos = i + 1;
                    skipping = false;
                    line_no += 1; // 버린 줄도 줄 번호는 센다
                }
                None => pos = data.len(),
            }
        }
        while let Some(i) = data[pos..].iter().position(|&b| b == b'\n') {
            let end = pos + i;
            let content_end = if end > pos && data[end - 1] == b'\r' {
                end - 1
            } else {
                end
            };
            line_no += 1;
            if end - pos > MAX_REMAINDER {
                dropped_lines += 1;
            } else {
                lines.push(Line {
                    offset: base + pos as u64,
                    line_no,
                    bytes: data[pos..content_end].to_vec(),
                });
            }
            pos = end + 1;
        }

        let remainder = if data.len() - pos > MAX_REMAINDER {
            dropped_lines += 1;
            skipping = true;
            pos = data.len();
            Vec::new()
        } else {
            data.split_off(pos)
        };

        // pos 직전 4KB. 부족하면 window 앞쪽(prev)에서 채운다
        let mut tail = Vec::with_capacity(TAIL_HASH_BYTES);
        let from_prev = TAIL_HASH_BYTES.saturating_sub(pos).min(prev.len());
        tail.extend_from_slice(&prev[prev.len() - from_prev..]);
        tail.extend_from_slice(&data[pos.saturating_sub(TAIL_HASH_BYTES - from_prev)..pos]);

        Ok(Pending {
            reset,
            lines,
            dropped_lines,
            generation,
            offset: base + pos as u64,
            remainder,
            tail_hash: if unchanged {
                self.tail_hash
            } else {
                hash_bytes(&tail)
            },
            line_no,
            skipping,
            len,
            mtime,
            swap_handle,
            file_id: if swap_handle {
                file_id(file)
            } else {
                self.file_id
            },
        })
    }

    /// 이벤트 적용이 끝난 뒤 호출자가 부른다. 이때만 상태를 전진시킨다
    pub fn commit(&mut self, p: Pending) {
        if p.swap_handle
            && let Some(f) = self.next.take()
        {
            self.file = f;
        }
        self.stalled = p.stalled_after(self.consumed());
        self.generation = p.generation;
        self.offset = p.offset;
        self.remainder = p.remainder;
        self.tail_hash = p.tail_hash;
        self.line_no = p.line_no;
        self.skipping = p.skipping;
        self.last_len = p.len;
        self.last_mtime = p.mtime;
        self.file_id = p.file_id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::Path;
    use std::time::Duration;

    fn append(path: &Path, bytes: &[u8]) {
        let mut f = OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(bytes).unwrap();
    }

    fn set_mtime(path: &Path, t: SystemTime) {
        let f = OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }

    /// 읽고 commit한 뒤 줄 내용만 돌려준다
    fn take(r: &mut TailReader) -> Pending {
        let p = r.read_new_lines(MAX_READ_BYTES).unwrap();
        let copy = Pending {
            lines: p.lines.clone(),
            remainder: p.remainder.clone(),
            ..p
        };
        r.commit(p);
        copy
    }

    fn texts(p: &Pending) -> Vec<&[u8]> {
        p.lines.iter().map(|l| &l.bytes[..]).collect()
    }

    fn setup(content: &[u8]) -> (tempfile::TempDir, PathBuf, TailReader) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        fs::write(&path, content).unwrap();
        let r = TailReader::open(&path).unwrap();
        (dir, path, r)
    }

    #[test]
    fn truncated_line_continues_next_read() {
        let (_d, path, mut r) = setup(b"a\r\nb");
        let p = take(&mut r);
        assert_eq!(texts(&p), [b"a"]); // \r 제거
        append(&path, b"c\nd\n");
        let p = take(&mut r);
        assert!(!p.reset);
        assert_eq!(texts(&p), [&b"bc"[..], b"d"]);
        // 시작 바이트 오프셋은 0 기준, 줄 번호는 1 기준
        assert_eq!((p.lines[0].offset, p.lines[0].line_no), (3, 2));
        assert_eq!((p.lines[1].offset, p.lines[1].line_no), (6, 3));
        assert!(!r.changed().unwrap());
    }

    #[test]
    fn backlog_continues_without_file_change() {
        let line = [vec![b'x'; 999], b"\n".to_vec()].concat();
        let (_d, path, mut r) = setup(b"");
        append(&path, &line.repeat(20_000)); // 약 20MB
        let mut total = 0;
        let mut reads = 0;
        while r.changed().unwrap() {
            total += take(&mut r).lines.len();
            reads += 1;
        }
        assert_eq!(total, 20_000);
        assert!(reads >= 3);
    }

    #[test]
    fn shrink_resets() {
        let (_d, path, mut r) = setup(b"one\ntwo\nthree\n");
        take(&mut r);
        fs::write(&path, b"x\n").unwrap();
        assert!(r.changed().unwrap());
        let p = r.read_new_lines(MAX_READ_BYTES).unwrap();
        assert!(p.reset);
        assert_eq!(r.generation(), 0); // commit 전에는 그대로
        r.commit(p);
        assert_eq!(r.generation(), 1);
    }

    #[test]
    fn shrink_into_remainder_resets() {
        // offset 5, remainder 2 바이트. 6바이트로 줄면 offset 보다는 크지만 읽어 둔 구간보다 짧다
        let (_d, path, mut r) = setup(b"aaaa\nbb");
        take(&mut r);
        fs::write(&path, b"aaaa\nb").unwrap();
        let p = take(&mut r);
        assert!(p.reset);
        assert_eq!(p.lines.len(), 1);
        assert_eq!(p.lines[0].offset, 0);
    }

    #[test]
    fn replaced_file_detected_by_path_check() {
        let (_d, path, mut r) = setup(b"old\n");
        take(&mut r);
        fs::rename(&path, path.with_extension("old")).unwrap();
        fs::write(&path, b"new1\nnew2\n").unwrap();
        r.verify_path().unwrap();
        // commit 없이 버려도 같은 결과
        let discarded = r.read_new_lines(MAX_READ_BYTES).unwrap();
        assert!(discarded.reset);
        let p = take(&mut r);
        assert!(p.reset);
        assert_eq!(texts(&p), [&b"new1"[..], b"new2"]);
        assert_eq!(r.generation(), 1);
        // 이후엔 새 파일을 따라간다
        r.verify_path().unwrap();
        append(&path, b"new3\n");
        let p = take(&mut r);
        assert!(!p.reset);
        assert_eq!(texts(&p), [b"new3"]);
    }

    #[test]
    fn same_length_overwrite_with_new_mtime_resets() {
        let (_d, path, mut r) = setup(b"aaaa\n");
        take(&mut r);
        fs::write(&path, b"bbbb\n").unwrap();
        set_mtime(&path, SystemTime::now() + Duration::from_secs(60));
        let p = take(&mut r);
        assert!(p.reset);
        assert_eq!(texts(&p), [b"bbbb"]);
    }

    #[test]
    fn overwritten_remainder_resets_on_growth() {
        let (_d, path, mut r) = setup(b"x\nabc");
        take(&mut r);
        fs::write(&path, b"x\nXYZ\nnext\n").unwrap();
        let p = take(&mut r);
        assert!(p.reset);
        assert_eq!(texts(&p), [&b"x"[..], b"XYZ", b"next"]);
    }

    #[test]
    fn tail_hash_catches_rewrite_before_offset() {
        let (_d, path, mut r) = setup(b"line1\nline2\n");
        take(&mut r);
        fs::write(&path, b"LINE1\nline2\nline3\n").unwrap();
        let p = take(&mut r);
        assert!(p.reset);
        assert_eq!(p.lines.len(), 3);
    }

    #[test]
    fn unchanged_read_keeps_tail_hash_so_append_is_not_reset() {
        let (_d, path, mut r) = setup(b"a\n");
        take(&mut r);
        take(&mut r); // 길이와 mtime 모두 그대로인 읽기
        append(&path, b"b\n");
        let p = take(&mut r);
        assert!(!p.reset);
        assert_eq!(texts(&p), [b"b"]);
    }

    #[test]
    fn rewrite_inside_last_4kb_resets_past_4kb_offset() {
        let line = format!("{}\n", "x".repeat(99));
        let (_d, path, mut r) = setup(line.repeat(100).as_bytes());
        take(&mut r);
        for _ in 0..3 {
            append(&path, line.as_bytes());
            assert!(!take(&mut r).reset);
        }
        assert!(r.offset > TAIL_HASH_BYTES as u64);
        // offset 직전 4KB 안쪽 한 바이트를 같은 길이로 덮어쓰고 길이는 늘린다
        let mut bytes = fs::read(&path).unwrap();
        bytes[r.offset as usize - 100] = b'Y';
        bytes.extend_from_slice(line.as_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(take(&mut r).reset);
    }

    #[test]
    fn line_over_64mb_is_dropped() {
        let (_d, path, mut r) = setup(b"first\n");
        take(&mut r);
        append(&path, &vec![b'x'; 70 * 1024 * 1024]);
        let (mut lines, mut dropped) = (Vec::new(), 0);
        while r.changed().unwrap() {
            let p = take(&mut r);
            dropped += p.dropped_lines;
            lines.extend(p.lines);
        }
        assert_eq!((lines.len(), dropped), (0, 1));
        append(&path, b"\nnext\n");
        let p = take(&mut r);
        assert!(!p.reset);
        assert_eq!(texts(&p), [b"next"]);
        assert_eq!(p.lines[0].line_no, 3); // first, 버린 줄, next
    }

    #[test]
    fn completed_line_over_64mb_is_dropped_at_read_boundary() {
        let (_d, path, mut r) = setup(b"");
        append(&path, &vec![b'x'; MAX_REMAINDER]);
        while r.changed().unwrap() {
            let p = r.read_new_lines(MAX_READ_BYTES).unwrap();
            assert!(p.lines.is_empty());
            assert_eq!(p.dropped_lines, 0);
            r.commit(p);
        }
        assert_eq!(r.unread_len(), MAX_REMAINDER);

        append(&path, b"x\nnext\n");
        let discarded = r.read_new_lines(MAX_READ_BYTES).unwrap();
        assert_eq!(discarded.dropped_lines, 1);
        assert_eq!(texts(&discarded), [b"next"]);
        let p = r.read_new_lines(MAX_READ_BYTES).unwrap();
        assert_eq!(p.dropped_lines, 1);
        assert_eq!(texts(&p), [b"next"]);
        assert_eq!(
            (p.lines[0].offset, p.lines[0].line_no),
            ((MAX_REMAINDER + 2) as u64, 2)
        );
        r.commit(p);
        assert!(!r.changed().unwrap());
    }

    #[test]
    fn completed_line_at_64mb_is_kept() {
        let (_d, path, mut r) = setup(b"");
        append(&path, &vec![b'x'; MAX_REMAINDER]);
        while r.changed().unwrap() {
            let p = r.read_new_lines(MAX_READ_BYTES).unwrap();
            r.commit(p);
        }

        append(&path, b"\nnext\n");
        let p = r.read_new_lines(MAX_READ_BYTES).unwrap();
        assert_eq!(p.dropped_lines, 0);
        assert_eq!(p.lines.len(), 2);
        assert_eq!(p.lines[0].bytes.len(), MAX_REMAINDER);
        assert_eq!((p.lines[0].offset, p.lines[0].line_no), (0, 1));
        assert_eq!(p.lines[1].bytes, b"next");
        assert_eq!(
            (p.lines[1].offset, p.lines[1].line_no),
            ((MAX_REMAINDER + 1) as u64, 2)
        );
    }

    #[test]
    fn discarded_pending_rereads_same_range() {
        let (_d, path, mut r) = setup(b"a\nb");
        let first = r.read_new_lines(MAX_READ_BYTES).unwrap(); // commit 안 하고 버림
        append(&path, b"c\n");
        let again = r.read_new_lines(MAX_READ_BYTES).unwrap();
        assert_eq!(
            first.lines,
            [Line {
                offset: 0,
                line_no: 1,
                bytes: b"a".to_vec()
            }]
        );
        assert_eq!(
            again
                .lines
                .iter()
                .map(|l| l.bytes.clone())
                .collect::<Vec<_>>(),
            [b"a".to_vec(), b"bc".to_vec()]
        );
        r.commit(again);
        assert!(!r.changed().unwrap());
    }

    #[test]
    fn finished_stops_when_read_does_not_advance() {
        // stat 길이가 실제로 읽히는 바이트보다 크게 보이는 경우를 파일로 만들 수 없어 Pending 의 len 을 부풀린다
        let (_d, _p, mut r) = setup(
            b"a
b
",
        );
        let mut p = r.read_new_lines(MAX_READ_BYTES).unwrap();
        p.len += 100;
        let mut prev = None;
        assert!(!p.finished(&mut prev), "처음 읽기는 비교할 직전이 없다");
        assert!(p.finished(&mut prev), "같은 끝이 되풀이되면 멈춘다");
        p.reset = true;
        assert!(!p.finished(&mut prev), "Reset 은 나아가지 못한 것이 아니다");
        p.len = 4;
        assert!(p.finished(&mut None), "파일 끝에 닿으면 멈춘다");
    }

    #[test]
    fn caught_up_when_inflated_length_cannot_be_read() {
        // finished_stops_when_read_does_not_advance 처럼 Pending 의 len 을 부풀려 stat 이 길이를 크게 말하는 경우를 만든다
        let (_d, _p, mut r) = setup(
            b"a
b
",
        );
        let mut p = r.read_new_lines(MAX_READ_BYTES).unwrap();
        p.len += 100;
        r.commit(p);
        assert!(
            !r.caught_up(r.last_len),
            "나아간 읽기 뒤 남은 길이는 덜 읽은 것이다"
        );
        let mut p = r.read_new_lines(MAX_READ_BYTES).unwrap();
        p.len += 100;
        r.commit(p);
        assert!(
            r.caught_up(r.last_len),
            "더 나아가지 못했으면 더 읽을 것이 없다"
        );
    }

    #[test]
    fn reads_file_held_open_for_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let mut writer = File::create(&path).unwrap();
        writer.write_all(b"a\n").unwrap();
        let mut r = TailReader::open(&path).unwrap();
        writer.write_all(b"b\n").unwrap();
        let p = take(&mut r);
        assert_eq!(texts(&p), [&b"a"[..], b"b"]);
    }

    #[test]
    fn growth_with_unchanged_mtime_is_read() {
        let (_d, path, mut r) = setup(b"a\n");
        let old = SystemTime::now() - Duration::from_secs(3600);
        set_mtime(&path, old);
        take(&mut r); // last_mtime = old
        append(&path, b"b\n");
        set_mtime(&path, old);
        assert!(r.changed().unwrap());
        let p = take(&mut r);
        assert!(!p.reset);
        assert_eq!(texts(&p), [b"b"]);
    }
}
