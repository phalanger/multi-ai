//! Screen-scrape fallback: identifies agent panes and infers agent state
//! from zellij screen dumps.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};

use mai_protocol::{AgentRule, AgentState, PaneRef, ScrapeRules};
use regex::Regex;

/// A rule pattern that failed to compile, with the agent it belongs to.
#[derive(Debug)]
pub struct RuleError {
    pub agent: String,
    pub pattern: String,
    pub source: regex::Error,
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "agent '{}': bad pattern '{}': {}",
            self.agent, self.pattern, self.source
        )
    }
}

impl std::error::Error for RuleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

struct CompiledRule {
    name: String,
    stable_ms: u64,
    title: Vec<Regex>,
    screen: Vec<Regex>,
    needs_input: Vec<Regex>,
    done: Vec<Regex>,
    ignore: Vec<Regex>,
}

fn compile_all(agent: &str, patterns: &[String]) -> Result<Vec<Regex>, RuleError> {
    patterns
        .iter()
        .map(|p| {
            Regex::new(p).map_err(|source| RuleError {
                agent: agent.to_owned(),
                pattern: p.clone(),
                source,
            })
        })
        .collect()
}

fn any_match(res: &[Regex], text: &str) -> bool {
    res.iter().any(|r| r.is_match(text))
}

impl CompiledRule {
    fn compile(r: &AgentRule) -> Result<Self, RuleError> {
        Ok(Self {
            name: r.name.clone(),
            stable_ms: r.stable_ms,
            title: compile_all(&r.name, &r.title_patterns)?,
            screen: compile_all(&r.name, &r.screen_patterns)?,
            needs_input: compile_all(&r.name, &r.needs_input_patterns)?,
            done: compile_all(&r.name, &r.done_patterns)?,
            ignore: compile_all(&r.name, &r.ignore_patterns)?,
        })
    }

    /// Screen text with every ignore-pattern match removed.
    fn clean<'a>(&self, screen: &'a str) -> Cow<'a, str> {
        let mut out = Cow::Borrowed(screen);
        for re in &self.ignore {
            if re.is_match(&out) {
                out = Cow::Owned(re.replace_all(&out, "").into_owned());
            }
        }
        out
    }
}

/// Rules with all regexes compiled once.
pub struct CompiledRules {
    rules: Vec<CompiledRule>,
}

impl CompiledRules {
    pub fn compile(src: &ScrapeRules) -> Result<Self, RuleError> {
        let rules = src
            .agents
            .iter()
            .map(CompiledRule::compile)
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    /// Name of the first agent whose title patterns match the pane title
    /// or command. Cheap: needs no screen dump.
    pub fn identify_by_title(&self, title: &str, command: Option<&str>) -> Option<&str> {
        self.rules
            .iter()
            .find(|r| any_match(&r.title, title) || command.is_some_and(|c| any_match(&r.title, c)))
            .map(|r| r.name.as_str())
    }

    /// Title/command match first; otherwise the first agent whose screen
    /// patterns match the screen.
    pub fn identify(&self, title: &str, command: Option<&str>, screen: &str) -> Option<&str> {
        self.identify_by_title(title, command).or_else(|| {
            self.rules
                .iter()
                .find(|r| any_match(&r.screen, screen))
                .map(|r| r.name.as_str())
        })
    }

    /// True when any of `agent`'s own rules still matches the pane: its
    /// title patterns (title or command), screen patterns, or its
    /// needs-input / done patterns. Prompts such as a plan confirmation
    /// may match none of the screen patterns yet still show the agent.
    pub fn still_matches(
        &self,
        agent: &str,
        title: &str,
        command: Option<&str>,
        screen: &str,
    ) -> bool {
        let Some(r) = self.rule(agent) else {
            return false;
        };
        if any_match(&r.title, title) || command.is_some_and(|c| any_match(&r.title, c)) {
            return true;
        }
        let screen = r.clean(screen);
        any_match(&r.screen, &screen)
            || any_match(&r.needs_input, &screen)
            || any_match(&r.done, &screen)
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
        let screen = rule.clean(screen);
        let hash = hash_screen(&screen);
        let Some(p) = self.panes.get_mut(pane) else {
            self.panes.insert(
                pane.clone(),
                PaneScrape {
                    hash,
                    stable_since_ms: now_ms,
                    emitted: None,
                },
            );
            return None;
        };
        let next = if p.hash != hash {
            p.hash = hash;
            p.stable_since_ms = now_ms;
            AgentState::Working
        } else if now_ms.saturating_sub(p.stable_since_ms) < rule.stable_ms {
            return None;
        } else if any_match(&rule.needs_input, &screen) {
            AgentState::NeedsInput
        } else if any_match(&rule.done, &screen) {
            AgentState::Done
        } else {
            p.emitted = None;
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

    /// Drop all history (used when rules are replaced).
    pub fn reset(&mut self) {
        self.panes.clear();
    }
}
