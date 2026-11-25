//! Execution context shared by every step: dry-run flag, log sink, and a record
//! of every side-effecting action (used by the zero-side-effect tests).
use crate::redact::redact;
use std::sync::{Arc, Mutex};

pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct Ctx {
    pub dry_run: bool,
    pub git_path: String,
    log: Arc<Mutex<Vec<String>>>,
    /// Every write action that was *performed* (not merely planned).
    performed: Arc<Mutex<Vec<String>>>,
    sink: Option<LogSink>,
}

impl Ctx {
    pub fn new(dry_run: bool) -> Self {
        Ctx {
            dry_run,
            git_path: "git".into(),
            log: Default::default(),
            performed: Default::default(),
            sink: None,
        }
    }
    pub fn with_git(mut self, git: Option<String>) -> Self {
        if let Some(g) = git.filter(|g| !g.trim().is_empty()) {
            self.git_path = g;
        }
        self
    }
    pub fn with_sink(mut self, sink: LogSink) -> Self {
        self.sink = Some(sink);
        self
    }

    pub fn log(&self, line: &str) {
        let line = redact(line);
        if let Some(s) = &self.sink {
            s(&line);
        }
        self.log.lock().unwrap().push(line);
    }

    /// Record a side-effecting action. In dry-run it is only logged as planned.
    /// Returns true when the caller should actually perform it.
    pub fn side_effect(&self, what: &str) -> bool {
        if self.dry_run {
            self.log(&format!("[dry-run] would {what}"));
            false
        } else {
            self.log(&format!("$ {what}"));
            self.performed.lock().unwrap().push(redact(what));
            true
        }
    }

    pub fn take_log(&self) -> String {
        std::mem::take(&mut *self.log.lock().unwrap()).join("\n")
    }
    pub fn performed(&self) -> Vec<String> {
        self.performed.lock().unwrap().clone()
    }
}
