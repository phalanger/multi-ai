//! `serve` core: turns spool records, zellij state and screen dumps into
//! `ProbeMsg`s, and applies `AppMsg`s. Apart from the `Zellij` trait and
//! the spool directory it does no IO, so tests drive it with a fake zellij.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Display;

use mai_protocol::{
    AgentEvent, AgentState, AppMsg, EventSource, PaneInfo, PaneRef, ProbeMsg, ScrapeRules,
    SessionInfo, decode_line,
};

use crate::hookmap::map_hook;
use crate::scrape::{CompiledRules, RuleError, ScrapeTracker};
use crate::spool::Spool;
use crate::zellij::Zellij;

/// Shortest interval `AppMsg::SetInterval` may set (one run-loop tick).
pub const MIN_INTERVAL_MS: u64 = 250;

/// Spool day files older than this many days are deleted.
pub const SPOOL_KEEP_DAYS: u64 = 7;

/// How often a long-running serve deletes old spool files.
pub const CLEANUP_EVERY_MS: u64 = 3_600_000;

/// A scrape-identified agent is reported `Exited` only after this many
/// consecutive scrapes in which none of its rules match the pane.
pub const MISSES_TO_EXIT: u8 = 2;

/// Polling intervals; changed by `AppMsg::SetInterval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intervals {
    pub pane_poll_ms: u64,
    pub scrape_ms: u64,
    pub metrics_ms: u64,
}

impl Default for Intervals {
    fn default() -> Self {
        Self {
            pane_poll_ms: 2_000,
            scrape_ms: 3_000,
            metrics_ms: 2_000,
        }
    }
}

/// `ProbeMsg::Error` with a stable machine-readable `code`.
pub fn error(code: &str, e: impl Display) -> ProbeMsg {
    ProbeMsg::Error {
        code: code.to_owned(),
        message: e.to_string(),
    }
}

/// True when `every_ms` has passed since `last` (or it never ran).
pub(crate) fn due(last: Option<u64>, every_ms: u64, now_ms: u64) -> bool {
    last.is_none_or(|t| now_ms.saturating_sub(t) >= every_ms)
}

/// Agent bound to a pane, and whether a hook (rather than scraping)
/// established the binding. Scrape bindings are re-checked every scrape.
struct Binding {
    agent: String,
    from_hook: bool,
    /// Consecutive scrapes in which none of the agent's rules matched.
    misses: u8,
}

/// What a periodic operation was doing when it failed. A failure is
/// reported once, when it starts; the scope must succeed again before a
/// new failure is reported.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Scope {
    Sessions,
    Panes(String),
    Dump(PaneRef),
    Cleanup,
}

/// Push `msg` unless `scope` is already failing.
fn report(failing: &mut HashSet<Scope>, scope: Scope, msg: ProbeMsg, out: &mut Vec<ProbeMsg>) {
    if failing.insert(scope) {
        out.push(msg);
    }
}

pub struct Server<Z> {
    zellij: Option<Z>,
    spool: Spool,
    rules: CompiledRules,
    scrape: ScrapeTracker,
    intervals: Intervals,
    read_cursor: u64,
    last_pane_poll: Option<u64>,
    last_scrape: Option<u64>,
    last_cleanup: Option<u64>,
    sessions: Vec<SessionInfo>,
    panes: BTreeMap<String, Vec<PaneInfo>>,
    /// Panes known to run an agent (from hooks or identification).
    agents: HashMap<PaneRef, Binding>,
    /// Panes whose agent a hook reported `Exited`. Scraping may bind them
    /// again only after one scrape in which no agent is identified, so a
    /// leftover title or last screen does not bring the agent back.
    exit_hold: HashSet<PaneRef>,
    failing: HashSet<Scope>,
    spool_failing: bool,
}

impl<Z: Zellij> Server<Z> {
    /// `zellij` is `None` when the binary was not found; the server then
    /// only relays spool events. Reading resumes after the last ack.
    pub fn new(zellij: Option<Z>, spool: Spool, rules: &ScrapeRules) -> Result<Self, RuleError> {
        Ok(Self {
            zellij,
            read_cursor: spool.load_ack(),
            spool,
            rules: CompiledRules::compile(rules)?,
            scrape: ScrapeTracker::default(),
            intervals: Intervals::default(),
            last_pane_poll: None,
            last_scrape: None,
            last_cleanup: None,
            sessions: Vec::new(),
            panes: BTreeMap::new(),
            agents: HashMap::new(),
            exit_hold: HashSet::new(),
            failing: HashSet::new(),
            spool_failing: false,
        })
    }

    pub fn intervals(&self) -> Intervals {
        self.intervals
    }

    /// One scheduling step: old spool files are deleted when due, then
    /// new spool records are read, then panes are polled and screens
    /// scraped when their intervals are due.
    pub fn tick(&mut self, now_ms: u64) -> Vec<ProbeMsg> {
        let mut out = Vec::new();
        if due(self.last_cleanup, CLEANUP_EVERY_MS, now_ms) {
            self.last_cleanup = Some(now_ms);
            match self.spool.cleanup(now_ms, SPOOL_KEEP_DAYS) {
                Ok(_) => {
                    self.failing.remove(&Scope::Cleanup);
                }
                Err(e) => report(
                    &mut self.failing,
                    Scope::Cleanup,
                    error("spool_cleanup", e),
                    &mut out,
                ),
            }
        }
        self.drain_spool(now_ms, &mut out);
        if self.zellij.is_some() {
            if due(self.last_pane_poll, self.intervals.pane_poll_ms, now_ms) {
                self.last_pane_poll = Some(now_ms);
                self.poll_panes(&mut out);
            }
            if due(self.last_scrape, self.intervals.scrape_ms, now_ms) {
                self.last_scrape = Some(now_ms);
                self.scrape_panes(now_ms, &mut out);
            }
        }
        out
    }

    fn drain_spool(&mut self, now_ms: u64, out: &mut Vec<ProbeMsg>) {
        let read = match self.spool.read_after(self.read_cursor, now_ms) {
            Ok(r) => r,
            Err(e) => {
                if !self.spool_failing {
                    out.push(error("spool_read", e));
                    self.spool_failing = true;
                }
                return;
            }
        };
        self.spool_failing = false;
        self.read_cursor = read.end_cursor;
        if read.bad_lines > 0 {
            out.push(error(
                "spool_bad_line",
                format!("skipped {} unparsable spool line(s)", read.bad_lines),
            ));
        }
        for (cursor, rec) in read.records {
            let pane = PaneRef {
                session: rec.session,
                pane_id: rec.pane_id,
            };
            let mapped = map_hook(&rec.agent, &rec.payload);
            if self.zellij.is_some() {
                if mapped
                    .as_ref()
                    .is_some_and(|m| m.state == AgentState::Exited)
                {
                    self.agents.remove(&pane);
                    self.scrape.forget(&pane);
                    self.exit_hold.insert(pane.clone());
                } else {
                    let binding = Binding {
                        agent: rec.agent.clone(),
                        from_hook: true,
                        misses: 0,
                    };
                    self.agents.insert(pane.clone(), binding);
                    self.exit_hold.remove(&pane);
                }
            }
            if let Some(m) = mapped {
                out.push(ProbeMsg::AgentEvent(AgentEvent {
                    pane,
                    agent: rec.agent,
                    source: EventSource::Hook,
                    state: m.state,
                    message: m.message,
                    ts_ms: rec.ts_ms,
                    spool_offset: Some(cursor),
                }));
            }
        }
    }

    fn poll_panes(&mut self, out: &mut Vec<ProbeMsg>) {
        let Some(z) = &self.zellij else { return };
        let sessions = match z.sessions() {
            Ok(s) => {
                self.failing.remove(&Scope::Sessions);
                s
            }
            Err(e) => {
                return report(&mut self.failing, Scope::Sessions, error("zellij", e), out);
            }
        };
        if sessions != self.sessions {
            out.push(ProbeMsg::Sessions {
                sessions: sessions.clone(),
            });
            self.sessions = sessions;
        }
        let mut fresh = BTreeMap::new();
        for s in self.sessions.iter().filter(|s| !s.exited) {
            let scope = Scope::Panes(s.name.clone());
            match z.panes(&s.name) {
                Ok(panes) => {
                    self.failing.remove(&scope);
                    if self.panes.get(&s.name) != Some(&panes) {
                        out.push(ProbeMsg::Panes {
                            session: s.name.clone(),
                            panes: panes.clone(),
                        });
                    }
                    fresh.insert(s.name.clone(), panes);
                }
                Err(e) => {
                    report(&mut self.failing, scope, error("zellij", e), out);
                    if let Some(old) = self.panes.get(&s.name) {
                        fresh.insert(s.name.clone(), old.clone());
                    }
                }
            }
        }
        let exists = |p: &PaneRef| {
            fresh
                .get(&p.session)
                .is_some_and(|ps: &Vec<PaneInfo>| ps.iter().any(|q| q.id == p.pane_id))
        };
        for (session, panes) in &self.panes {
            for p in panes {
                let pane = PaneRef {
                    session: session.clone(),
                    pane_id: p.id,
                };
                if !exists(&pane) {
                    self.scrape.forget(&pane);
                }
            }
        }
        self.agents.retain(|k, _| exists(k));
        self.exit_hold.retain(|k| exists(k));
        let live = &self.sessions;
        self.failing.retain(|s| match s {
            Scope::Panes(name) => live.iter().any(|l| !l.exited && &l.name == name),
            Scope::Dump(p) => exists(p),
            Scope::Sessions | Scope::Cleanup => true,
        });
        self.panes = fresh;
    }

    fn scrape_panes(&mut self, now_ms: u64, out: &mut Vec<ProbeMsg>) {
        let Self {
            zellij,
            rules,
            scrape,
            agents,
            panes,
            exit_hold,
            failing,
            ..
        } = self;
        let Some(z) = zellij else { return };
        for (session, list) in panes.iter() {
            for p in list.iter().filter(|p| !p.exited) {
                let pane = PaneRef {
                    session: session.clone(),
                    pane_id: p.id,
                };
                let scope = Scope::Dump(pane.clone());
                let screen = match z.dump_screen(session, p.id) {
                    Ok(s) => {
                        failing.remove(&scope);
                        s
                    }
                    Err(e) => {
                        report(failing, scope, error("zellij", e), out);
                        continue;
                    }
                };
                let command = p.command.as_deref();
                let found = rules.identify(&p.title, command, &screen);
                let agent = match agents.get_mut(&pane) {
                    Some(b) if b.from_hook => b.agent.clone(),
                    Some(b) => {
                        if found == Some(b.agent.as_str())
                            || rules.still_matches(&b.agent, &p.title, command, &screen)
                        {
                            b.misses = 0;
                            b.agent.clone()
                        } else {
                            b.misses += 1;
                            if b.misses < MISSES_TO_EXIT {
                                continue;
                            }
                            // Agent is gone from a scrape-identified pane
                            // (e.g. back at the shell): report it once.
                            out.push(scrape_event(&pane, &b.agent, AgentState::Exited, now_ms));
                            agents.remove(&pane);
                            scrape.forget(&pane);
                            continue;
                        }
                    }
                    None => {
                        let Some(a) = found else {
                            exit_hold.remove(&pane);
                            continue;
                        };
                        if exit_hold.contains(&pane) {
                            continue;
                        }
                        let binding = Binding {
                            agent: a.to_owned(),
                            from_hook: false,
                            misses: 0,
                        };
                        agents.insert(pane.clone(), binding);
                        out.push(scrape_event(&pane, a, AgentState::Unknown, now_ms));
                        a.to_owned()
                    }
                };
                if let Some(state) = scrape.observe(rules, &agent, &pane, &screen, now_ms) {
                    out.push(scrape_event(&pane, &agent, state, now_ms));
                }
            }
        }
    }

    /// Apply one line read from stdin. A bad line is reported, not fatal.
    pub fn handle_line(&mut self, line: &str) -> Vec<ProbeMsg> {
        if line.trim().is_empty() {
            return Vec::new();
        }
        match decode_line::<AppMsg>(line) {
            Ok(msg) => self.handle(msg),
            Err(e) => {
                let head: String = line.chars().take(80).collect();
                vec![error("bad_message", format!("{e}: {head}"))]
            }
        }
    }

    pub fn handle(&mut self, msg: AppMsg) -> Vec<ProbeMsg> {
        match msg {
            AppMsg::Ack { spool_offset } => self
                .spool
                .store_ack(spool_offset)
                .err()
                .map(|e| error("spool_ack", e))
                .into_iter()
                .collect(),
            AppMsg::SendText {
                session,
                pane_id,
                text,
            } => self.with_zellij(|z| z.paste(&session, pane_id, &text)),
            AppMsg::Focus {
                session,
                pane_id,
                tab_id,
            } => self.with_zellij(|z| z.focus(&session, tab_id, pane_id)),
            AppMsg::SetInterval {
                pane_poll_ms,
                scrape_ms,
                metrics_ms,
            } => {
                self.intervals = Intervals {
                    pane_poll_ms: pane_poll_ms.max(MIN_INTERVAL_MS),
                    scrape_ms: scrape_ms.max(MIN_INTERVAL_MS),
                    metrics_ms: metrics_ms.max(MIN_INTERVAL_MS),
                };
                Vec::new()
            }
            AppMsg::SetRules { rules } => match CompiledRules::compile(&rules) {
                Ok(compiled) => {
                    self.rules = compiled;
                    self.scrape.reset();
                    Vec::new()
                }
                Err(e) => vec![error("bad_rules", e)],
            },
        }
    }

    fn with_zellij<E: Display>(&self, f: impl FnOnce(&Z) -> Result<(), E>) -> Vec<ProbeMsg> {
        match &self.zellij {
            None => vec![error("zellij_missing", "zellij not found on this host")],
            Some(z) => f(z).err().map(|e| error("zellij", e)).into_iter().collect(),
        }
    }
}

fn scrape_event(pane: &PaneRef, agent: &str, state: AgentState, now_ms: u64) -> ProbeMsg {
    ProbeMsg::AgentEvent(AgentEvent {
        pane: pane.clone(),
        agent: agent.to_owned(),
        source: EventSource::Scrape,
        state,
        message: None,
        ts_ms: now_ms,
        spool_offset: None,
    })
}
