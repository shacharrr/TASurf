//! Command line: tasurf <command> ... (tasurf/__main__.py)
//!
//!   bake          timeline JSON -> baked per-tick file (and copy into the game)
//!   verify        compare run CSVs bit for bit (determinism check)
//!   check         compare a run CSV against the reference air-movement model
//!   build-plugin  compile plugin/scripting/tasurf.sp with spcomp
//!   install       copy the compiled plugin and baked files into the game
//!   editor        open the browser editor (per-tick editing, path, speed, best yaw)
//!   import-recording  turn a tas_record recording into an editable timeline

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tasurf::config::{self, pypath, repo_root, Config};
use tasurf::{bake, editdoc, editor, io_error, physics, py, runcsv, timeline};

const PROG: &str = "tasurf";

const DESCRIPTION: &str = "Command line: tasurf <command> ...

  bake          timeline JSON -> baked per-tick file (and copy into the game)
  verify        compare run CSVs bit for bit (determinism check)
  check         compare a run CSV against the reference air-movement model
  build-plugin  compile plugin/scripting/tasurf.sp with spcomp
  install       copy the compiled plugin and baked files into the game
  editor        open the browser editor (per-tick editing, path, speed, best yaw)
  import-recording  turn a tas_record recording into an editable timeline";

fn fail(msg: &str) -> u8 {
    eprintln!("error: {}", msg);
    2
}

fn default_out() -> PathBuf {
    repo_root().join("out").join("baked")
}

fn plugin_src() -> PathBuf {
    repo_root().join("plugin").join("scripting").join("tasurf.sp")
}

fn plugin_out() -> PathBuf {
    repo_root().join("plugin").join("plugins").join("tasurf.smx")
}

// --------------------------------------------------------------------------
// a small argparse: positionals, --flags and --options with values

struct Spec {
    name: &'static str,
    help: &'static str,
    positionals: &'static [(&'static str, Arity, &'static str)],
    options: &'static [(&'static str, Option<&'static str>, Option<&'static str>, &'static str)], // long, short, metavar (None = flag), help
}

#[derive(Clone, Copy, PartialEq)]
enum Arity {
    One,
    Optional,
    OneOrMore,
    Any,
}

#[derive(Default)]
struct Args {
    positionals: Vec<(String, Vec<String>)>,
    values: Vec<(String, String)>,
    flags: Vec<String>,
}

impl Args {
    fn pos(&self, name: &str) -> Vec<String> {
        self.positionals.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap_or_default()
    }

    fn one(&self, name: &str) -> Option<String> {
        self.pos(name).into_iter().next()
    }

    fn value(&self, name: &str) -> Option<String> {
        self.values.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }
}

const SPECS: &[Spec] = &[
    Spec {
        name: "bake",
        help: "bake timeline JSON files",
        positionals: &[("timelines", Arity::OneOrMore, "")],
        options: &[
            ("--out-dir", Some("-o"), Some("OUT_DIR"), ""),
            ("--no-install", None, None, "don't copy the baked file into the game"),
        ],
    },
    Spec {
        name: "verify",
        help: "compare run CSVs bit for bit",
        positionals: &[("csv", Arity::Any, "")],
        options: &[
            ("--latest", None, Some("NAME"), "compare the newest runs of this timeline from the game's runs folder"),
            ("--count", None, Some("COUNT"), "with --latest: how many recent runs (default 2)"),
            ("--runs-dir", None, Some("RUNS_DIR"), ""),
            (
                "--rows-only",
                None,
                None,
                "compare tick rows only; list header differences without failing (e.g. a live recording against its replay)",
            ),
        ],
    },
    Spec {
        name: "check",
        help: "compare a run against the reference air-movement model",
        positionals: &[("csv", Arity::One, "")],
        options: &[],
    },
    Spec {
        name: "build-plugin",
        help: "compile the SourceMod plugin",
        positionals: &[],
        options: &[("--spcomp", None, Some("SPCOMP"), "")],
    },
    Spec {
        name: "install",
        help: "copy plugin + baked files into the game",
        positionals: &[],
        options: &[("--no-baked", None, None, "")],
    },
    Spec {
        name: "editor",
        help: "open the browser editor",
        positionals: &[("timeline", Arity::Optional, "timeline to open (name or path)")],
        options: &[
            ("--port", None, Some("PORT"), "0 picks a free port"),
            ("--no-browser", None, None, ""),
        ],
    },
    Spec {
        name: "import-recording",
        help: "turn a recording into an editable timeline",
        positionals: &[("recording", Arity::One, "name in the game's baked folder (e.g. surf_beginner_rec) or a .tas path")],
        options: &[
            ("--name", None, Some("NAME"), "timeline name (default: <recording>_edit)"),
            ("--tolerance", None, Some("TOLERANCE"), "degrees a yaw/pitch keyframe may deviate to save keys (0 = exact)"),
            ("--force", None, None, ""),
        ],
    },
];

fn main_usage() -> String {
    let names: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
    format!("usage: {} [-h] {{{}}} ...", PROG, names.join(","))
}

fn main_help() -> String {
    let mut out = format!("{}\n\n{}\n\npositional arguments:\n  {{{}}}\n", main_usage(), DESCRIPTION,
                          SPECS.iter().map(|s| s.name).collect::<Vec<_>>().join(","));
    for s in SPECS {
        out.push_str(&format!("    {:<16} {}\n", s.name, s.help));
    }
    out.push_str("\noptions:\n  -h, --help            show this help message and exit\n");
    out
}

fn sub_usage(spec: &Spec) -> String {
    let mut parts = vec![format!("usage: {} {} [-h]", PROG, spec.name)];
    for (long, short, metavar, _) in spec.options {
        let flag = short.unwrap_or(long);
        parts.push(match metavar {
            Some(m) => format!("[{} {}]", flag, m),
            None => format!("[{}]", flag),
        });
    }
    for (name, arity, _) in spec.positionals {
        parts.push(match arity {
            Arity::One => name.to_string(),
            Arity::Optional => format!("[{}]", name),
            Arity::OneOrMore => format!("{} [{} ...]", name, name),
            Arity::Any => format!("[{} ...]", name),
        });
    }
    parts.join(" ")
}

fn sub_help(spec: &Spec) -> String {
    let mut out = format!("{}\n", sub_usage(spec));
    if !spec.positionals.is_empty() {
        out.push_str("\npositional arguments:\n");
        for (name, _, help) in spec.positionals {
            out.push_str(&format!("{}\n", format!("  {:<22}{}", name, help).trim_end()));
        }
    }
    out.push_str("\noptions:\n  -h, --help            show this help message and exit\n");
    for (long, short, metavar, help) in spec.options {
        let mut flag = match short {
            Some(s) => format!("{}, {}", s, long),
            None => long.to_string(),
        };
        if let Some(m) = metavar {
            flag = match short {
                Some(s) => format!("{} {}, {} {}", s, m, long, m),
                None => format!("{} {}", long, m),
            };
        }
        if flag.len() + 2 > 24 {
            out.push_str(&format!("  {}\n", flag));
            if !help.is_empty() {
                out.push_str(&format!("                        {}\n", help));
            }
        } else {
            out.push_str(&format!("{}\n", format!("  {:<22}{}", flag, help).trim_end()));
        }
    }
    out
}

enum Parsed {
    Run(&'static Spec, Args),
    Exit(u8),
}

fn usage_error(usage: &str, msg: &str) -> Parsed {
    eprintln!("{}\n{}: error: {}", usage, PROG, msg);
    Parsed::Exit(2)
}

fn parse_args(argv: &[String]) -> Parsed {
    let first = match argv.first() {
        Some(a) => a.as_str(),
        None => return usage_error(&main_usage(), "the following arguments are required: cmd"),
    };
    if first == "-h" || first == "--help" {
        print!("{}", main_help());
        return Parsed::Exit(0);
    }
    let spec = match SPECS.iter().find(|s| s.name == first) {
        Some(s) => s,
        None => {
            let choices: Vec<String> = SPECS.iter().map(|s| format!("'{}'", s.name)).collect();
            return usage_error(
                &main_usage(),
                &format!("argument cmd: invalid choice: '{}' (choose from {})", first, choices.join(", ")),
            );
        }
    };
    let usage = sub_usage(spec);
    let mut args = Args::default();
    let mut loose: Vec<String> = Vec::new();
    let mut i = 1;
    let mut only_positionals = false;
    while i < argv.len() {
        let a = &argv[i];
        i += 1;
        if only_positionals || !a.starts_with('-') || a == "-" {
            loose.push(a.clone());
            continue;
        }
        if a == "--" {
            only_positionals = true;
            continue;
        }
        if a == "-h" || a == "--help" {
            print!("{}", sub_help(spec));
            return Parsed::Exit(0);
        }
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if a.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        // --long, a unique prefix of it, or the short form (-oDIR too)
        let mut found = spec.options.iter().find(|o| o.0 == flag || o.1 == Some(flag.as_str()));
        let mut inline = inline;
        if found.is_none() && flag.starts_with("--") {
            let matches: Vec<_> = spec.options.iter().filter(|o| o.0.starts_with(flag.as_str())).collect();
            if matches.len() == 1 {
                found = Some(matches[0]);
            }
        }
        if found.is_none() && !flag.starts_with("--") && flag.len() > 2 {
            if let Some(o) = spec.options.iter().find(|o| o.1 == Some(&flag[..2]) && o.2.is_some()) {
                found = Some(o);
                inline = Some(flag[2..].to_string());
            }
        }
        let (long, _, metavar, _) = match found {
            Some(o) => *o,
            None => return usage_error(&usage, &format!("unrecognized arguments: {}", a)),
        };
        match metavar {
            None => args.flags.push(long.to_string()),
            Some(_) => {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        if i >= argv.len() || (argv[i].starts_with('-') && argv[i].len() > 1 && argv[i].parse::<f64>().is_err()) {
                            let shown = match found.and_then(|o| o.1) {
                                Some(s) => format!("{}/{}", s, long),
                                None => long.to_string(),
                            };
                            return usage_error(&usage, &format!("argument {}: expected one argument", shown));
                        }
                        i += 1;
                        argv[i - 1].clone()
                    }
                };
                args.values.push((long.to_string(), v));
            }
        }
    }
    // hand the loose arguments to the positionals
    let mut rest = loose.into_iter();
    let mut leftover: Vec<String> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for (name, arity, _) in spec.positionals {
        let vals: Vec<String> = match arity {
            Arity::One => rest.next().into_iter().collect(),
            Arity::Optional => rest.next().into_iter().collect(),
            Arity::OneOrMore | Arity::Any => rest.by_ref().collect(),
        };
        if vals.is_empty() && (*arity == Arity::One || *arity == Arity::OneOrMore) {
            missing.push(name);
        }
        args.positionals.push((name.to_string(), vals));
    }
    leftover.extend(rest);
    if !missing.is_empty() {
        return usage_error(&usage, &format!("the following arguments are required: {}", missing.join(", ")));
    }
    if !leftover.is_empty() {
        return usage_error(&usage, &format!("unrecognized arguments: {}", leftover.join(" ")));
    }
    Parsed::Run(spec, args)
}

fn typed<T: std::str::FromStr>(args: &Args, long: &str, kind: &str, default: T) -> Result<T, u8> {
    match args.value(long) {
        None => Ok(default),
        Some(v) => v.trim().parse::<T>().map_err(|_| {
            let spec_usage = "";
            let _ = spec_usage;
            eprintln!("{}: error: argument {}: invalid {} value: '{}'", PROG, long, kind, v);
            2
        }),
    }
}

// --------------------------------------------------------------------------
// commands

fn cmd_bake(args: &Args, cfg: &Config) -> u8 {
    let data_dir = cfg.game_data_dir();
    let search: Vec<PathBuf> = data_dir.iter().map(|d| d.join("starts")).collect();
    let out_dir = args.value("--out-dir").map(|d| pypath(&d)).unwrap_or_else(default_out);
    let mut status = 0;
    for tl_path in args.pos("timelines") {
        let result = timeline::load_timeline(&pypath(&tl_path), &search).map_err(|e| e.0).and_then(|tl| {
            let out = out_dir.join(format!("{}.tas", tl.name));
            let (text, inputs) = bake::bake_to_file(&tl, &out, None)?;
            Ok((tl, out, text, inputs))
        });
        let (tl, out, text, inputs) = match result {
            Ok(x) => x,
            Err(e) => {
                eprintln!("error: {}: {}", tl_path, e);
                status = 2;
                continue;
            }
        };
        for line in bake::summarize(&tl, &inputs) {
            println!("{}", line);
        }
        for w in &tl.warnings {
            println!("  warning: {}", w);
        }
        println!("  wrote {} (hash {})", out.display(), bake::hash_of(&text));
        if let Some(d) = &data_dir {
            if !args.flag("--no-install") {
                let dest = d.join("baked").join(out.file_name().unwrap_or_default());
                let copied = std::fs::create_dir_all(d.join("baked")).and_then(|_| std::fs::copy(&out, &dest));
                if let Err(e) = copied {
                    eprintln!("error: {}", io_error(&e, &dest));
                    return 1;
                }
                println!("  installed {}", dest.display());
                println!("  in game: tas_load {}   then   tas_run", tl.name);
            }
        }
    }
    status
}

/// found[-count:] as Python slices it.
fn last_n<T: Clone>(v: &[T], count: i64) -> Vec<T> {
    let n = v.len() as i64;
    let start = if count == 0 { 0 } else if count > 0 { (n - count).max(0) } else { (-count).min(n) };
    v[start as usize..].to_vec()
}

fn cmd_verify(args: &Args, cfg: &Config) -> u8 {
    let mut paths: Vec<PathBuf> = args.pos("csv").iter().map(|p| pypath(p)).collect();
    let count = match typed::<i64>(args, "--count", "int", 2) {
        Ok(c) => c,
        Err(code) => return code,
    };
    if let Some(latest) = args.value("--latest") {
        let runs_dir = match args.value("--runs-dir") {
            Some(d) if !d.is_empty() => Some(pypath(&d)),
            _ => cfg.game_data_dir().map(|d| d.join("runs")),
        };
        let runs_dir = match runs_dir {
            Some(d) => d,
            None => return fail("--latest needs --runs-dir or cstrike_dir in tasurf.local.json"),
        };
        let found = runcsv::find_runs(&runs_dir, &latest);
        if found.len() < 2 {
            return fail(&format!(
                "need at least 2 runs of {} in {}, found {}",
                py::repr_str(&latest),
                runs_dir.display(),
                found.len()
            ));
        }
        paths = last_n(&found, count);
    }
    if paths.len() < 2 {
        return fail("give at least two CSV files, or --latest NAME");
    }
    let mut texts = Vec::new();
    let mut runs = Vec::new();
    for p in &paths {
        let text = match std::fs::read_to_string(p) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: {}", io_error(&e, p));
                return 1;
            }
        };
        match runcsv::parse_run(&text, Some(p)) {
            Ok(r) => runs.push(r),
            Err(e) => {
                eprintln!("error: {}", e);
                return 1;
            }
        }
        texts.push(text);
    }
    let name = |p: &Path| editor::file_name(p);
    let mut all_same = true;
    println!(
        "reference: {} ({} ticks, status {})",
        paths[0].display(),
        runs[0].rows.len(),
        runs[0].get("status").unwrap_or("?")
    );
    for k in 1..paths.len() {
        let mut cmp = runcsv::compare_runs(&runs[0], &runs[k], Some(&texts[0]), Some(&texts[k]));
        if args.flag("--rows-only")
            && !cmp.header_diffs.is_empty()
            && !cmp.header_diffs.iter().any(|d| d.starts_with("columns differ"))
        {
            // e.g. a live recording (status "recorded") against its replay (status "complete")
            for d in &cmp.header_diffs {
                println!("  (header, ignored) {}", d);
            }
            cmp.header_diffs.clear();
            cmp.identical = cmp.first_diff_tick.is_none() && cmp.length_note.is_none();
            if cmp.identical {
                println!("IDENTICAL ROWS: {} matches {} tick for tick", name(&paths[k]), name(&paths[0]));
                continue;
            }
        }
        all_same &= cmp.identical;
        for line in cmp.lines(&name(&paths[0]), &name(&paths[k])) {
            println!("{}", line);
        }
    }
    if all_same {
        0
    } else {
        1
    }
}

fn cmd_check(args: &Args, _cfg: &Config) -> u8 {
    let path = pypath(&args.one("csv").unwrap_or_default());
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {}", io_error(&e, &path));
            return 1;
        }
    };
    let run = match runcsv::parse_run(&text, Some(&path)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {}", e);
            return 1;
        }
    };
    let rep = match physics::check_run(&run) {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    for line in rep.lines() {
        println!("{}", line);
    }
    if rep.mismatches.is_empty() {
        0
    } else {
        1
    }
}

fn find_spcomp(args: &Args, cfg: &Config) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(s) = args.value("--spcomp").filter(|s| !s.is_empty()) {
        candidates.push(pypath(&s));
    }
    if let Some(s) = cfg.spcomp() {
        candidates.push(s);
    }
    let mut roots = vec![repo_root().join("third_party").join("sourcemod")];
    if let Some(c) = cfg.cstrike_dir() {
        roots.push(c);
    }
    for root in roots {
        let scripting = root.join("addons").join("sourcemod").join("scripting");
        for exe in ["spcomp64.exe", "spcomp.exe", "spcomp64", "spcomp"] {
            candidates.push(scripting.join(exe));
        }
    }
    candidates.into_iter().find(|c| c.is_file())
}

fn cmd_build_plugin(args: &Args, cfg: &Config) -> u8 {
    let spcomp = match find_spcomp(args, cfg) {
        Some(s) => s,
        None => {
            return fail(
                "spcomp not found. Pass --spcomp PATH, set TASURF_SPCOMP, or extract the SourceMod package into \
                 third_party/sourcemod (see README).",
            )
        }
    };
    let include = spcomp.parent().map(|p| p.join("include")).unwrap_or_default();
    let (src, out) = (plugin_src(), plugin_out());
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let cmd = [
        spcomp.display().to_string(),
        src.display().to_string(),
        format!("-i{}", include.display()),
        format!("-o{}", out.display()),
    ];
    println!("{}", cmd.iter().map(|c| if c.contains(' ') { format!("\"{}\"", c) } else { c.clone() }).collect::<Vec<_>>().join(" "));
    let status = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .current_dir(src.parent().unwrap_or(Path::new(".")))
        .status();
    let code = match status {
        Ok(s) => s.code().unwrap_or(-1),
        Err(e) => {
            eprintln!("error: {}", io_error(&e, &spcomp));
            return 1;
        }
    };
    if code != 0 || !out.is_file() {
        return fail(&format!("spcomp failed (exit code {})", code));
    }
    println!("built {}", out.display());
    0
}

fn copy_or_fail(from: &Path, to: &Path) -> Result<(), u8> {
    std::fs::copy(from, to).map(|_| ()).map_err(|e| {
        eprintln!("error: {}", io_error(&e, to));
        1
    })
}

fn cmd_install(args: &Args, cfg: &Config) -> u8 {
    let cstrike = match cfg.cstrike_dir() {
        Some(c) => c,
        None => return fail("set cstrike_dir in tasurf.local.json (or TASURF_CSTRIKE)"),
    };
    let sm = cstrike.join("addons").join("sourcemod");
    if !sm.is_dir() {
        return fail(&format!("{} does not exist; install Metamod:Source and SourceMod first (see README)", sm.display()));
    }
    let out = plugin_out();
    if !out.is_file() {
        return fail(&format!("{} does not exist; run: tasurf build-plugin", out.display()));
    }
    let data = cfg.game_data_dir().unwrap_or_default();
    for sub in ["baked", "starts", "runs"] {
        let _ = std::fs::create_dir_all(data.join(sub));
    }
    let dest = sm.join("plugins").join(out.file_name().unwrap_or_default());
    if let Err(c) = copy_or_fail(&out, &dest) {
        return c;
    }
    println!("installed {}", dest.display());
    let session_cfg = repo_root().join("plugin").join("cfg").join("tasurf_session.cfg");
    let cfg_dest = cstrike.join("cfg").join("tasurf_session.cfg");
    if let Err(c) = copy_or_fail(&session_cfg, &cfg_dest) {
        return c;
    }
    println!("installed {}", cfg_dest.display());
    let baked_dir = default_out();
    if !args.flag("--no-baked") && baked_dir.is_dir() {
        let mut files = editor::glob_files(&baked_dir, "tas");
        files.sort();
        for f in files {
            let dest = data.join("baked").join(f.file_name().unwrap_or_default());
            if let Err(c) = copy_or_fail(&f, &dest) {
                return c;
            }
            println!("installed {}", dest.display());
        }
    }
    println!("in game: sm plugins reload tasurf   (or change map)");
    0
}

fn cmd_editor(args: &Args, _cfg: &Config) -> u8 {
    let port = match typed::<u16>(args, "--port", "int", 8765) {
        Ok(p) => p,
        Err(code) => return code,
    };
    match editor::serve(args.one("timeline").as_deref(), port, !args.flag("--no-browser")) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {}", e);
            1
        }
    }
}

fn cmd_import_recording(args: &Args, cfg: &Config) -> u8 {
    let tolerance = match typed::<f64>(args, "--tolerance", "float", 0.0) {
        Ok(t) => t,
        Err(code) => return code,
    };
    let recording = args.one("recording").unwrap_or_default();
    let mut src = pypath(&recording);
    if !src.is_file() {
        let cand = cfg.game_data_dir().map(|d| d.join("baked").join(format!("{}.tas", recording)));
        match cand {
            Some(c) if c.is_file() => src = c,
            _ => {
                return fail(&format!(
                    "no recording {} (give a .tas path or a name from the game's baked folder)",
                    py::repr_str(&recording)
                ))
            }
        }
    }
    let name = match args.value("--name").filter(|n| !n.is_empty()) {
        Some(n) => n,
        None => {
            let st = editor::stem(&src);
            format!("{}_edit", st.strip_suffix("_rec").unwrap_or(&st))
        }
    };
    let out = repo_root().join("timelines").join(format!("{}.json", name));
    if out.exists() && !args.flag("--force") {
        return fail(&format!("{} exists (use --name or --force)", out.display()));
    }
    let rep = match editdoc::import_recording(&src, &out, &name, tolerance) {
        Ok(r) => r,
        Err(e) => return fail(&e.0),
    };
    let n = |k: &str| rep[k].as_i64().unwrap_or(0);
    println!(
        "wrote {}: {} ticks, {} yaw keys, {} pitch keys, {} key ranges, move speed {}",
        out.display(),
        n("ticks"),
        n("yaw_keys"),
        n("pitch_keys"),
        n("key_ranges"),
        py::g(rep["move_speed"].as_f64().unwrap_or(0.0))
    );
    if rep["exact"].as_bool() == Some(true) {
        println!("  exact: it bakes back to the recorded inputs bit for bit");
    } else {
        println!("  {} ticks differ from the recording:", n("differing_ticks"));
        for line in rep["first_differences"].as_array().into_iter().flatten() {
            println!("    {}", line.as_str().unwrap_or(""));
        }
    }
    0
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (spec, args) = match parse_args(&argv) {
        Parsed::Run(spec, args) => (spec, args),
        Parsed::Exit(code) => return ExitCode::from(code),
    };
    let cfg = config::load();
    let code = match spec.name {
        "bake" => cmd_bake(&args, &cfg),
        "verify" => cmd_verify(&args, &cfg),
        "check" => cmd_check(&args, &cfg),
        "build-plugin" => cmd_build_plugin(&args, &cfg),
        "install" => cmd_install(&args, &cfg),
        "editor" => cmd_editor(&args, &cfg),
        "import-recording" => cmd_import_recording(&args, &cfg),
        _ => 2,
    };
    ExitCode::from(code)
}
