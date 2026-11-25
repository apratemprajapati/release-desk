//! Start and stop a local self-hosted Actions runner folder (`run.cmd` on
//! Windows, `run.sh` elsewhere). Registration (`config.cmd`) stays manual: it
//! needs a one-time token from GitHub and is a deliberate admin step.
use crate::error::{Error, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

#[derive(Default)]
pub struct Runners(Mutex<HashMap<String, Child>>);

#[derive(Debug, Serialize)]
pub struct LocalRunner {
    pub dir: String,
    pub exists: bool,
    pub configured: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub log_file: String,
}

fn script(dir: &Path) -> PathBuf {
    if cfg!(windows) { dir.join("run.cmd") } else { dir.join("run.sh") }
}

fn log_path(dir: &Path) -> PathBuf {
    dir.join("_diag").join("release-desk-runner.log")
}

impl Runners {
    pub fn status(&self, dir: &str) -> LocalRunner {
        let d = Path::new(dir);
        let mut map = self.0.lock().unwrap();
        let mut pid = None;
        if let Some(c) = map.get_mut(dir) {
            match c.try_wait() {
                Ok(None) => pid = Some(c.id()),
                _ => { map.remove(dir); }
            }
        }
        LocalRunner {
            dir: dir.into(),
            exists: script(d).exists(),
            configured: d.join(".runner").exists(),
            running: pid.is_some(),
            pid,
            log_file: log_path(d).to_string_lossy().into(),
        }
    }

    pub fn start(&self, dir: &str) -> Result<LocalRunner> {
        let d = Path::new(dir);
        let s = script(d);
        if !s.exists() {
            return Err(Error::Other(format!("{} not found: point this at an extracted actions-runner folder", s.display())));
        }
        if !d.join(".runner").exists() {
            return Err(Error::Other("runner is not configured yet: run config.cmd once (see GitHub → Settings → Actions → Runners → New runner)".into()));
        }
        if self.status(dir).running {
            return Ok(self.status(dir));
        }
        std::fs::create_dir_all(d.join("_diag"))?;
        let log = std::fs::OpenOptions::new().create(true).append(true).open(log_path(d))?;
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&s);
            c
        } else {
            Command::new(&s)
        };
        cmd.current_dir(d).stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd.spawn()?;
        self.0.lock().unwrap().insert(dir.to_string(), child);
        Ok(self.status(dir))
    }

    /// Stop the runner and its child processes (Runner.Listener / Worker).
    pub fn stop(&self, dir: &str) -> Result<LocalRunner> {
        let child = self.0.lock().unwrap().remove(dir);
        if let Some(mut c) = child {
            let pid = c.id().to_string();
            if cfg!(windows) {
                let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &pid]).output();
            } else {
                let _ = Command::new("kill").args(["-TERM", &format!("-{pid}")]).output();
            }
            let _ = c.kill();
            let _ = c.wait();
        }
        Ok(self.status(dir))
    }

    pub fn tail_log(&self, dir: &str, lines: usize) -> String {
        let text = std::fs::read_to_string(log_path(Path::new(dir))).unwrap_or_default();
        let v: Vec<&str> = text.lines().collect();
        crate::redact::redact(&v[v.len().saturating_sub(lines)..].join("\n"))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn starts_and_stops_a_fake_runner() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().to_string_lossy().to_string();
        let r = Runners::default();
        assert!(!r.status(&dir).exists);
        std::fs::write(d.path().join("run.sh"), "#!/bin/sh\necho listening\nexec sleep 60\n").unwrap();
        std::process::Command::new("chmod").arg("+x").arg(d.path().join("run.sh")).status().unwrap();
        assert!(r.start(&dir).is_err(), "unconfigured runner refused");
        std::fs::write(d.path().join(".runner"), "{}").unwrap();
        let s = r.start(&dir).unwrap();
        assert!(s.running && s.configured);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(r.tail_log(&dir, 5).contains("listening"));
        let s = r.stop(&dir).unwrap();
        assert!(!s.running);
    }
}
