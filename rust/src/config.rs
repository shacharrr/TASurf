//! Local machine configuration (tasurf/config.py): tasurf.local.json in the repo root,
//! overridden by the environment variables TASURF_CSTRIKE and TASURF_SPCOMP.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub cstrike_dir: Option<String>,
    pub spcomp: Option<String>,
}

/// The TASurf folder: TASURF_ROOT, else the first folder up from the current one or
/// from this program that holds plugin/scripting/tasurf.sp, else the current folder.
pub fn repo_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        if let Ok(r) = std::env::var("TASURF_ROOT") {
            if !r.is_empty() {
                return native(&r);
            }
        }
        let mut starts = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            starts.push(cwd);
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                starts.push(dir.to_path_buf());
            }
        }
        for start in &starts {
            let mut dir: Option<&Path> = Some(start.as_path());
            while let Some(d) = dir {
                if d.join("plugin").join("scripting").join("tasurf.sp").is_file() {
                    return d.to_path_buf();
                }
                dir = d.parent();
            }
        }
        starts.into_iter().next().unwrap_or_else(|| PathBuf::from("."))
    })
}

/// A path as Python's pathlib shows it on this system (backslashes on Windows).
pub fn native(p: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(p.replace('/', "\\"))
    } else {
        PathBuf::from(p)
    }
}

/// `Path(s)` as pathlib normalises it: native separators, no doubled separators,
/// no `.` parts and no trailing separator.
pub fn pypath(s: &str) -> PathBuf {
    let p = native(s);
    let mut out = PathBuf::new();
    for c in p.components() {
        if c != std::path::Component::CurDir {
            out.push(c.as_os_str());
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

pub fn load() -> Config {
    let mut cfg = Config::default();
    let path = repo_root().join("tasurf.local.json");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&text) {
            cfg.cstrike_dir = o.get("cstrike_dir").and_then(Value::as_str).map(str::to_string);
            cfg.spcomp = o.get("spcomp").and_then(Value::as_str).map(str::to_string);
        }
    }
    if let Ok(v) = std::env::var("TASURF_CSTRIKE") {
        if !v.is_empty() {
            cfg.cstrike_dir = Some(v);
        }
    }
    if let Ok(v) = std::env::var("TASURF_SPCOMP") {
        if !v.is_empty() {
            cfg.spcomp = Some(v);
        }
    }
    cfg
}

fn resolve(v: &Option<String>) -> Option<PathBuf> {
    let v = v.as_deref().filter(|s| !s.is_empty())?;
    let p = native(v);
    Some(if p.is_absolute() { p } else { repo_root().join(p) })
}

impl Config {
    pub fn cstrike_dir(&self) -> Option<PathBuf> {
        resolve(&self.cstrike_dir)
    }

    pub fn spcomp(&self) -> Option<PathBuf> {
        resolve(&self.spcomp)
    }

    /// addons/sourcemod/data/tasurf inside the game, where the plugin reads and writes.
    pub fn game_data_dir(&self) -> Option<PathBuf> {
        self.cstrike_dir().map(|c| c.join("addons").join("sourcemod").join("data").join("tasurf"))
    }
}
