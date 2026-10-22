# 0.2.0 릴리스 대기

`v0.2.0`의 개발·출시 전 검증과 전체 리뷰 수정을 통합했다. 사용자 지시대로 **main 병합 전에 정지**한다. main 병합·정식 태그·릴리스·배포는 실행하지 않았다.

## 남은 순서

1. main 병합 재개 지시를 받은 뒤 `docs/BUILD.md` 릴리스 절차를 따른다. 서명 키의 별도 백업과 개발 도구가 없는 Windows 11 설치 검증은 남아 있다.
2. T82-2 정식 출시 후 기존 0.1.x 사용자는 설치 파일을 직접 덮어 설치한다.
3. T82-3에서 공개 GitHub HTTPS·redirect 경로와 다음 버전 자동 업데이트를 확인한다. 그 뒤 두 PLAN과 이 진행 파일을 지운다.

정식 `v0.2.0` 태그는 아직 없어 `release_check.py`는 마지막 태그 검사에서 실패한다. 버전 셋·설치 파일명·서명 일치·manifest·README 링크 검사는 통과했다. 태그 검사를 피하려고 시험 태그를 만들지 않는다.

## 다음 작업에 필요한 위치

- 서명된 로컬 설치 후보: `D:/workspace/owlfice-wt/t91-target/release/bundle/nsis/Owlfice_0.2.0_x64-setup.exe`와 옆의 `.sig`. main 확정 뒤 릴리스 절차대로 다시 확인한다.
- 공개 키는 정식 설정에 있다. 개인 키·DPAPI 비밀번호는 저장소 밖 `%USERPROFILE%/.tauri/`에 있으며 출력하거나 저장소에 넣지 않는다.
- `%TEMP%/owlfice-updater-qa-current.txt`: test.1/test.2 설치·서명·실패·중복 방지·설정 보존 검증 자료. private 저장소 `dbwk10317/owlfice-updater-test`는 유지하되 시험 릴리스와 시험 설치 앱은 제거했다. 토큰은 앱에 넣지 않았다. 연결 실패는 PC 전체 네트워크를 끄지 않고 로컬 endpoint 중단으로 재현했다.
- `%TEMP%/owlfice-readme-qa-current.txt`: 실제 0.2.0 앱의 합성 세션과 README PNG 원본·후보 manifest. `refresh.py`는 검수 앱을 종료한 뒤 실행한다. 야근 이미지는 실제 수동 야근 모드이며 조명 시각을 조작하지 않았다.
- 화면 검수 범위는 `design/HANDOFF.md` 12절. 실제 OS 배율은 150%이며 DPR emulation과 구분한다. Windows 키보드·마우스로 설정 Tab 순환·스크롤, 설정·직원 상세의 Esc 닫기·포커스 복귀, 야근 모드, 타임라인 펼침을 확인했다. 로그 폴더 열기는 표시한 경로와 같은 탐색기 폴더를 열었다. 리뷰 수정 후 실행 파일에서 좁은 머리말·야근 대비·드로어·미니맵 간격·세션 상세를 재검수했다. 설정 업데이트 절은 합성 진행 상태로도 확인할 수 있다 (`dev/settings.html`).

임시 정리: 메인이 잘못 지정한 `D:/workspace/owlfice-wt/t91-daylight/src-tauri/t91-target` 빌드 폴더 삭제가 자동 승인 검토에서 정책상 거부됐다. 커밋에는 포함하지 않았으며 재시도하지 않는다.
