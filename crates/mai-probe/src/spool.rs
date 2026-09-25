//! Spool: append-only event log. Hook processes append records; `serve`
//! reads them back in order and reports them to the app.
//!
//! Files are `<dir>/<day>.jsonl`, where `<day>` is days since the Unix
//! epoch (UTC). A record's cursor is `(day << 40) | end_offset`, where
//! `end_offset` is the byte offset just past the record's newline, so
//! cursors increase strictly across all files.
//!
//! A hook stamps its record just before appending it, so a record stamped
//! at 23:59:59.999 can land in yesterday's file just after midnight. The
//! reader therefore leaves a new day's file alone for `DAY_GRACE_MS`
//! after that day starts; once it moves on to a day, earlier files are
//! never read again.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DAY_MS: u64 = 86_400_000;
/// How long after midnight (UTC) a new day's file is left unread.
pub const DAY_GRACE_MS: u64 = 10_000;
const OFFSET_BITS: u32 = 40;
const OFFSET_MASK: u64 = (1 << OFFSET_BITS) - 1;

/// One hook invocation as written by `mai-probe hook` / `emit`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpoolRecord {
    pub ts_ms: u64,
    pub agent: String,
    pub session: String,
    pub pane_id: u32,
    pub payload: serde_json::Value,
}

/// Records after a cursor, the number of unparsable lines skipped, and
/// the cursor just past the last complete line read (good or bad).
#[derive(Debug, Default)]
pub struct ReadResult {
    pub records: Vec<(u64, SpoolRecord)>,
    pub bad_lines: usize,
    pub end_cursor: u64,
}

pub fn make_cursor(day: u64, end_offset: u64) -> u64 {
    (day << OFFSET_BITS) | (end_offset & OFFSET_MASK)
}

fn split_cursor(cursor: u64) -> (u64, u64) {
    (cursor >> OFFSET_BITS, cursor & OFFSET_MASK)
}

pub struct Spool {
    dir: PathBuf,
    /// Name of the file holding the acknowledged cursor.
    ack_file: String,
}

pub use mai_protocol::sanitize_client;

impl Spool {
    /// Spool whose acknowledged cursor is stored in `ack`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            ack_file: "ack".to_owned(),
        }
    }

    /// Spool whose acknowledged cursor belongs to one app installation
    /// (`ack-<client>`), so two apps watching the same host do not
    /// consume each other's events on restart.
    pub fn for_client(dir: impl Into<PathBuf>, client: &str) -> Self {
        let client = sanitize_client(client);
        if client.is_empty() {
            return Self::new(dir);
        }
        Self {
            dir: dir.into(),
            ack_file: format!("ack-{client}"),
        }
    }

    fn day_file(&self, day: u64) -> PathBuf {
        self.dir.join(format!("{day}.jsonl"))
    }

    /// Append one record as a single write, so concurrent hook processes
    /// do not interleave within a line.
    pub fn append(&self, rec: &SpoolRecord) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut line = serde_json::to_vec(rec)?;
        line.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.day_file(rec.ts_ms / DAY_MS))?;
        f.write_all(&line)
    }

    /// Spool days present on disk, ascending.
    fn days(&self) -> io::Result<Vec<u64>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e),
        };
        let mut days = Vec::new();
        for entry in entries {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(day) = name.strip_suffix(".jsonl").and_then(|d| d.parse().ok()) {
                days.push(day);
            }
        }
        days.sort_unstable();
        Ok(days)
    }

    /// All complete records whose cursor is greater than `cursor`.
    /// A trailing line without a newline (still being written) is left
    /// for the next call, and so is any day after the cursor's day that
    /// began less than `DAY_GRACE_MS` before `now_ms`.
    pub fn read_after(&self, cursor: u64, now_ms: u64) -> io::Result<ReadResult> {
        let (cur_day, cur_off) = split_cursor(cursor);
        let mut out = ReadResult {
            end_cursor: cursor,
            ..ReadResult::default()
        };
        for day in self.days()?.into_iter().filter(|d| *d >= cur_day) {
            if day > cur_day && now_ms < (day * DAY_MS).saturating_add(DAY_GRACE_MS) {
                break;
            }
            let start = if day == cur_day { cur_off } else { 0 };
            let mut f = File::open(self.day_file(day))?;
            f.seek(SeekFrom::Start(start))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            let mut offset = start;
            for chunk in buf.split_inclusive(|b| *b == b'\n') {
                if chunk.last() != Some(&b'\n') {
                    break;
                }
                offset += chunk.len() as u64;
                let at = make_cursor(day, offset);
                match serde_json::from_slice(&chunk[..chunk.len() - 1]) {
                    Ok(rec) => out.records.push((at, rec)),
                    Err(_) => out.bad_lines += 1,
                }
                out.end_cursor = at;
            }
        }
        Ok(out)
    }

    /// Last cursor acknowledged by the app; 0 when none.
    pub fn load_ack(&self) -> u64 {
        fs::read_to_string(self.dir.join(&self.ack_file))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Write the cursor via a temp file named after this process, so two
    /// serves never share a temp file.
    pub fn store_ack(&self, cursor: u64) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = self
            .dir
            .join(format!("{}.{}.tmp", self.ack_file, std::process::id()));
        fs::write(&tmp, cursor.to_string())?;
        fs::rename(tmp, self.dir.join(&self.ack_file))
    }

    /// Delete day files older than `keep_days` before `now_ms`.
    /// Returns how many files were removed.
    pub fn cleanup(&self, now_ms: u64, keep_days: u64) -> io::Result<usize> {
        let today = now_ms / DAY_MS;
        let mut removed = 0;
        for day in self.days()? {
            if day + keep_days < today {
                fs::remove_file(self.day_file(day))?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Default spool directory under the probe home (`<home>/.mai/spool`).
pub fn spool_dir(mai_home: &Path) -> PathBuf {
    mai_home.join("spool")
}
