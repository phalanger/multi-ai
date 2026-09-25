use std::fs::OpenOptions;
use std::io::Write;

use mai_probe::spool::{DAY_GRACE_MS, Spool, SpoolRecord, make_cursor, sanitize_client};
use serde_json::json;

const DAY: u64 = 86_400_000;
/// A clock far past every test day, so no day file is still in its grace period.
const LATER: u64 = u64::MAX;

fn rec(ts_ms: u64, pane: u32) -> SpoolRecord {
    SpoolRecord {
        ts_ms,
        agent: "claude".into(),
        session: "work".into(),
        pane_id: pane,
        payload: json!({"hook_event_name": "Stop"}),
    }
}

#[test]
fn append_then_read_all_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(10 * DAY + 1, 1)).unwrap();
    s.append(&rec(10 * DAY + 2, 2)).unwrap();
    s.append(&rec(11 * DAY + 1, 3)).unwrap();
    let r = s.read_after(0, LATER).unwrap();
    let panes: Vec<u32> = r.records.iter().map(|(_, x)| x.pane_id).collect();
    assert_eq!(panes, vec![1, 2, 3]);
    assert_eq!(r.bad_lines, 0);
    let cursors: Vec<u64> = r.records.iter().map(|(c, _)| *c).collect();
    assert!(cursors.windows(2).all(|w| w[0] < w[1]), "{cursors:?}");
    assert_eq!(cursors[2] >> 40, 11);
}

#[test]
fn read_after_cursor_skips_seen_records() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(5 * DAY, 1)).unwrap();
    s.append(&rec(5 * DAY, 2)).unwrap();
    let first = s.read_after(0, LATER).unwrap().records[0].0;
    let rest = s.read_after(first, LATER).unwrap();
    assert_eq!(rest.records.len(), 1);
    assert_eq!(rest.records[0].1.pane_id, 2);
    let last = rest.records[0].0;
    assert!(s.read_after(last, LATER).unwrap().records.is_empty());
}

#[test]
fn partial_trailing_line_waits_for_newline() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(7 * DAY, 1)).unwrap();
    let path = dir.path().join("7.jsonl");
    let mut f = OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(b"{\"ts_ms\":1").unwrap();
    let r = s.read_after(0, LATER).unwrap();
    assert_eq!(r.records.len(), 1);
    assert_eq!(r.bad_lines, 0);
    assert_eq!(
        r.records[0].0,
        make_cursor(7, std::fs::metadata(&path).unwrap().len() - 10)
    );
}

#[test]
fn bad_lines_are_counted_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    std::fs::write(dir.path().join("3.jsonl"), b"not json\n").unwrap();
    s.append(&rec(3 * DAY, 9)).unwrap();
    let r = s.read_after(0, LATER).unwrap();
    assert_eq!(r.bad_lines, 1);
    assert_eq!(r.records.len(), 1);
    assert_eq!(r.end_cursor, r.records[0].0);
    std::fs::write(dir.path().join("4.jsonl"), b"also bad\n").unwrap();
    let r = s.read_after(r.end_cursor, LATER).unwrap();
    assert_eq!((r.records.len(), r.bad_lines), (0, 1));
    let again = s.read_after(r.end_cursor, LATER).unwrap();
    assert_eq!((again.records.len(), again.bad_lines), (0, 0));
    assert_eq!(again.end_cursor, r.end_cursor);
}

#[test]
fn missing_dir_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path().join("nope"));
    assert!(s.read_after(0, LATER).unwrap().records.is_empty());
    assert_eq!(s.load_ack(), 0);
}

#[test]
fn ack_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.store_ack(make_cursor(4, 123)).unwrap();
    assert_eq!(s.load_ack(), make_cursor(4, 123));
}

#[test]
fn cleanup_removes_only_old_days() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(DAY, 1)).unwrap();
    s.append(&rec(9 * DAY, 2)).unwrap();
    s.store_ack(1).unwrap();
    let removed = s.cleanup(10 * DAY, 7).unwrap();
    assert_eq!(removed, 1);
    let left: Vec<u32> = s
        .read_after(0, LATER)
        .unwrap()
        .records
        .iter()
        .map(|(_, r)| r.pane_id)
        .collect();
    assert_eq!(left, vec![2]);
    assert_eq!(s.load_ack(), 1);
}

#[test]
fn new_day_file_waits_for_grace_period() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(20 * DAY + 5, 1)).unwrap();
    let first = s.read_after(0, 20 * DAY + DAY_GRACE_MS).unwrap();
    assert_eq!(first.records.len(), 1);

    // Just after midnight a record lands in the new day's file, and a
    // late hook stamped before midnight lands in the old one.
    s.append(&rec(21 * DAY + 1, 2)).unwrap();
    s.append(&rec(21 * DAY - 1, 3)).unwrap();
    let early = s.read_after(first.end_cursor, 21 * DAY + 100).unwrap();
    let panes: Vec<u32> = early.records.iter().map(|(_, r)| r.pane_id).collect();
    assert_eq!(
        panes,
        vec![3],
        "new day's file is left alone during the grace period"
    );

    let later = s
        .read_after(early.end_cursor, 21 * DAY + DAY_GRACE_MS)
        .unwrap();
    let panes: Vec<u32> = later.records.iter().map(|(_, r)| r.pane_id).collect();
    assert_eq!(panes, vec![2]);
}

#[test]
fn each_client_has_its_own_ack() {
    let dir = tempfile::tempdir().unwrap();
    let a = Spool::for_client(dir.path(), "mac-app");
    let b = Spool::for_client(dir.path(), "win-app");
    a.store_ack(make_cursor(1, 10)).unwrap();
    assert_eq!(a.load_ack(), make_cursor(1, 10));
    assert_eq!(b.load_ack(), 0);
    assert_eq!(Spool::new(dir.path()).load_ack(), 0);
    assert!(dir.path().join("ack-mac-app").is_file());
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn client_ids_are_sanitized_for_file_names() {
    assert_eq!(sanitize_client(r"a/b\c:d e_f-1"), "abcde_f-1");
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::for_client(dir.path(), "../..");
    s.store_ack(7).unwrap();
    assert_eq!(
        Spool::new(dir.path()).load_ack(),
        7,
        "empty id falls back to `ack`"
    );
}
