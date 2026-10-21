//! 로거는 프로세스에 하나라 단위 테스트와 섞이지 않게 따로 돈다.
//! 실제 기록 자리(등록부 읽기 실패)가 초기화된 로거를 거쳐 파일에 남는지 본다

use owlfice_lib::discover::registry::read_registry;
use owlfice_lib::oplog;

#[test]
fn 등록부_읽기_실패는_로그_파일에_한_번만_남고_내용은_없다() {
    let tmp = tempfile::tempdir().unwrap();
    let logs = tmp.path().join("logs");
    oplog::init(&logs);

    let sessions = tmp.path().join("home").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("1.json"), "SECRET-BODY{not json").unwrap();
    let home = tmp.path().join("home");
    for _ in 0..3 {
        assert!(read_registry(&home).unwrap().entries.is_empty());
    }

    let file = std::fs::read_dir(&logs).unwrap().next().unwrap().unwrap();
    let text = std::fs::read_to_string(file.path()).unwrap();
    assert_eq!(text.matches("등록부 파일 해석 실패 1.json").count(), 1);
    assert!(!text.contains("SECRET-BODY"));
}
