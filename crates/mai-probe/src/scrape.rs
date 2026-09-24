//! Screen-scrape fallback: identifies agent panes and infers agent state
//! from zellij screen dumps.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use mai_protocol::{AgentRule, AgentState, PaneRef, ScrapeRules};
use regex::Regex;

struct CompiledRule {
    name: String,
    stable_ms: u64,
    title: Vec<Regex>,
    screen: Vec<Regex>,
    needs_input: Vec<Regex>,
    done: Vec<Regex>,
}

fn compile_all(patterns: &[String]) -> Result<Vec<Regex>, regex::Error> {
    patterns.iter().map(|p| Regex::new(p)).collect()
}

fn any_match(res: &[Regex], text: &str) -> bool {
    res.iter().any(|r| r.is_match(text))
}

impl CompiledRule {
    fn compile(r: &AgentRule) -> Result<Self, regex::Error> {
        Ok(Self {
            name: r.name.clone(),
            stable_ms: r.stable_ms,
            title: compile_all(&r.title_patterns)?,
            screen: compile_all(&r.screen_patterns)?,
            needs_input: compile_all(&r.needs_input_patterns)?,
            done: compile_all(&r.done_patterns)?,
        })
    }
}

/// Rules with all regexes compiled once.
pub struct CompiledRules {
    rules: Vec<CompiledRule>,
}

impl CompiledRules {
    pub fn compile(src: &ScrapeRules) -> Result<Self, regex::Error> {
        let rules = src
            .agents
            .iter()
            .map(CompiledRule::compile)
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    /// Name of the first agent whose title patterns match the pane title
    /// or command, or whose screen patterns match the screen.
    pub fn identify(
        &self,
        title: &str,
        command: Option<&str>,
        screen: &str,
    ) -> Option<&str> {
        self.rules
            .iter()
            .find(|r| {
                any_match(&r.title, title)
                    || command.is_some_and(|c| any_match(&r.title, c))
                    || any_match(&r.screen, screen)
            })
            .map(|r| r.name.as_str())
    }

    fn rule(&self, name: &str) -> Option<&CompiledRule> {
        self.rules.iter().find(|r| r.name == name)
    }
}

struct PaneScrape {
    hash: u64,
    stable_since_ms: u64,
    emitted: Option<AgentState>,
}

/// Per-pane screen history used to infer state changes.
#[derive(Default)]
pub struct ScrapeTracker {
    panes: HashMap<PaneRef, PaneScrape>,
}

fn hash_screen(screen: &str) -> u64 {
    let mut h = DefaultHasher::new();
    screen.hash(&mut h);
    h.finish()
}

impl ScrapeTracker {
    /// Feed one screen dump; returns a state only when it changes.
    pub fn observe(
        &mut self,
        rules: &CompiledRules,
        agent: &str,
        pane: &PaneRef,
        screen: &str,
        now_ms: u64,
    ) -> Option<AgentState> {
        let rule = rules.rule(agent)?;
        let hash = hash_screen(screen);
        let Some(p) = self.panes.get_mut(pane) else {
            self.panes.insert(
                pane.clone(),
                PaneScrape { hash, stable_since_ms: now_ms, emitted: None },
            );
            return None;
        };
        let next = if p.hash != hash {
            p.hash = hash;
            p.stable_since_ms = now_ms;
            AgentState::Working
        } else if now_ms.saturating_sub(p.stable_since_ms) < rule.stable_ms {
            return None;
        } else if any_match(&rule.needs_input, screen) {
            AgentState::NeedsInput
        } else if any_match(&rule.done, screen) {
            AgentState::Done
        } else {
            return None;
        };
        if p.emitted == Some(next) {
            return None;
        }
        p.emitted = Some(next);
        Some(next)
    }

    /// Drop history for a pane that no longer exists.
    pub fn forget(&mut self, pane: &PaneRef) {
        self.panes.remove(pane);
    }
}
