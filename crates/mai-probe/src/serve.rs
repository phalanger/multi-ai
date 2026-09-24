//! `serve` core: turns spool records, zellij state and screen dumps into
//! `ProbeMsg`s, and applies `AppMsg`s. Apart from the `Zellij` trait and
//! the spool directory it does no IO, so tests drive it with a fake zellij.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;

use mai_protocol::{
    AgentEvent, AgentState, AppMsg, EventSource, PaneInfo, PaneRef, ProbeMsg, ScrapeRules,
    SessionInfo, decode_line,
};

use crate::hookmap::map_hook;
use crate::scrape::{CompiledRules, RuleError, ScrapeTracker};
use crate::spool::Spool;
use crate::zellij::Zellij;

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
    sessions: Vec<SessionInfo>,
    panes: BTreeMap<String, Vec<PaneInfo>>,
    /// Panes known to run an agent (from hooks or identification).
    agents: HashMap<PaneRef, Binding>,
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
            sessions: Vec::new(),
            panes: BTreeMap::new(),
            agents: HashMap::new(),
            spool_failing: false,
        })
    }

    pub fn intervals(&self) -> Intervals {
        self.intervals
    }

    /// One scheduling step: new spool records, then pane polling and
    /// screen scraping when their intervals are due.
    pub fn tick(&mut self, now_ms: u64) -> Vec<ProbeMsg> {
        let mut out = Vec::new();
        self.drain_spool(&mut out);
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

    fn drain_spool(&mut self, out: &mut Vec<ProbeMsg>) {
        let read = match self.spool.read_after(self.read_cursor) {
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
                } else {
                    let binding = Binding {
                        agent: rec.agent.clone(),
                        from_hook: true,
                    };
                    self.agents.insert(pane.clone(), binding);
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
            Ok(s) => s,
            Err(e) => return out.push(error("zellij", e)),
        };
        if sessions != self.sessions {
            out.push(ProbeMsg::Sessions {
                sessions: sessions.clone(),
            });
            self.sessions = sessions;
        }
        let mut fresh = BTreeMap::new();
        for s in self.sessions.iter().filter(|s| !s.exited) {
            match z.panes(&s.name) {
                Ok(panes) => {
                    if self.panes.get(&s.name) != Some(&panes) {
                        out.push(ProbeMsg::Panes {
                            session: s.name.clone(),
                            panes: panes.clone(),
                        });
                    }
                    fresh.insert(s.name.clone(), panes);
                }
                Err(e) => {
                    out.push(error("zellij", e));
                    if let Some(old) = self.panes.get(&s.name) {
                        fresh.insert(s.name.clone(), old.clone());
                    }
                }
            }
        }
        for (session, panes) in &self.panes {
            for p in panes {
                let still = fresh
                    .get(session)
                    .is_some_and(|ps| ps.iter().any(|q| q.id == p.id));
                if !still {
                    let gone = PaneRef {
                        session: session.clone(),
                        pane_id: p.id,
                    };
                    self.scrape.forget(&gone);
                }
            }
        }
        self.agents.retain(|k, _| {
            fresh
                .get(&k.session)
                .is_some_and(|ps| ps.iter().any(|p| p.id == k.pane_id))
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
            ..
        } = self;
        let Some(z) = zellij else { return };
        for (session, list) in panes.iter() {
            for p in list.iter().filter(|p| !p.exited) {
                let pane = PaneRef {
                    session: session.clone(),
                    pane_id: p.id,
                };
                let screen = match z.dump_screen(session, p.id) {
                    Ok(s) => s,
                    Err(e) => {
                        out.push(error("zellij", e));
                        continue;
                    }
                };
                let found = rules.identify(&p.title, p.command.as_deref(), &screen);
                let agent = match agents.get(&pane) {
                    Some(b) if b.from_hook || found == Some(b.agent.as_str()) => b.agent.clone(),
                    Some(b) => {
                        // Agent is gone from a scrape-identified pane (e.g.
                        // back at the shell): report it once and unbind.
                        out.push(scrape_event(&pane, &b.agent, AgentState::Exited, now_ms));
                        agents.remove(&pane);
                        scrape.forget(&pane);
                        continue;
                    }
                    None => {
                        let Some(a) = found else { continue };
                        let binding = Binding {
                            agent: a.to_owned(),
                            from_hook: false,
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
                    pane_poll_ms,
                    scrape_ms,
                    metrics_ms,
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
