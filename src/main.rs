mod scan;
mod tlpdb;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio, exit};
use std::env;

const HISTORIC: &str = "https://ftp.math.utah.edu/pub/tex/historic/systems/texlive";
const ROLLING: &str = "https://mirror.ctan.org/systems/texlive/tlnet";
const BASE: [&str; 2] = ["latex-bin", "bibtex"];
#[cfg(all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"))]
const PLATFORM: &str = "x86_64-linux";
#[cfg(all(target_os = "linux", target_env = "gnu", target_arch = "aarch64"))]
const PLATFORM: &str = "aarch64-linux";
#[cfg(not(all(target_os = "linux", target_env = "gnu", any(target_arch = "x86_64", target_arch = "aarch64"))))]
compile_error!("texup supports x86_64 and aarch64 Linux with glibc only");
const FONTS_CONF: &str = "texup-fonts.conf";

macro_rules! die {
    ($($t:tt)*) => {{ eprintln!("texup: {}", format!($($t)*)); exit(1) }};
}

#[derive(Deserialize)]
struct Manifest {
    texlive: TexLive,
    #[serde(default)]
    build: Build,
}
#[derive(Deserialize)]
struct TexLive {
    year: u32,
}
#[derive(Deserialize, Default)]
struct Build {
    main: Option<String>,
    engine: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct Lock {
    version: u32,
    texlive: String,
    #[serde(default)]
    package: Vec<LockPkg>,
}
#[derive(Serialize, Deserialize)]
struct LockPkg {
    name: String,
    revision: u64,
}

struct Project {
    engine: &'static str,
    base: Vec<&'static str>,
    dir: PathBuf,
    main: String,
    year: u32,
    tl: PathBuf,
}

impl Project {
    fn file(&self, ext: &str) -> PathBuf {
        self.dir.join(Path::new(&self.main).with_extension(ext))
    }
}

fn home() -> PathBuf {
    env::var_os("TEXUP_HOME").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env::var_os("HOME").unwrap_or_else(|| die!("$HOME is not set"))).join(".cache/texup")
    })
}

fn read(p: &Path) -> Option<String> {
    fs::read(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn ok(c: &mut Command) -> bool {
    match c.status() {
        Ok(s) => s.success(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            die!("{} not found; install it with your package manager", c.get_program().to_string_lossy())
        }
        Err(e) => die!("{}: {e}", c.get_program().to_string_lossy()),
    }
}

fn bin(tl: &Path) -> PathBuf {
    tl.join("bin").join(PLATFORM)
}

fn check(c: &mut Command) {
    if !ok(c) {
        die!("{:?} failed", c.get_program());
    }
}

fn tex(tl: &Path, prog: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(prog);
    let path = env::var("PATH").unwrap_or_default();
    c.env("PATH", format!("{}:{path}", bin(tl).display())).env("max_print_line", "10000");
    c.env("FONTCONFIG_FILE", tl.join(FONTS_CONF));
    c
}

// Regenerate fontconfig paths so XeTeX finds fonts after an installation is moved.
fn ready(tl: PathBuf) -> PathBuf {
    let tl = fs::canonicalize(&tl).unwrap_or(tl);
    let fonts = tl.join("texmf-dist/fonts");
    let conf = format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n<fontconfig>\n\
         <include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include>\n\
         <dir>{0}/opentype</dir>\n<dir>{0}/truetype</dir>\n<dir>{0}/type1</dir>\n</fontconfig>\n",
        fonts.display()
    );
    let path = tl.join(FONTS_CONF);
    if read(&path).as_deref() != Some(conf.as_str()) {
        fs::write(&path, conf).unwrap_or_else(|e| die!("{e}"));
    }
    tl
}

// An absolute path lets tlmgr locate its installation.
fn tlmgr_install<S: AsRef<std::ffi::OsStr>>(tl: &Path, pkgs: impl IntoIterator<Item = S>) {
    let log = home().join("tlmgr.log");
    let f = fs::OpenOptions::new().create(true).append(true).open(&log).unwrap_or_else(|e| die!("{e}"));
    let out = f.try_clone().unwrap_or_else(|e| die!("{e}"));
    let start = f.metadata().map_or(0, |m| m.len() as usize);
    if !ok(tex(tl, bin(tl).join("tlmgr")).arg("install").args(pkgs).stdout(out).stderr(f)) {
        let bytes = fs::read(&log).unwrap_or_default();
        let text = String::from_utf8_lossy(bytes.get(start..).unwrap_or_default());
        let lines: Vec<_> = text.lines().collect();
        eprintln!("{}", lines[lines.len().saturating_sub(20)..].join("\n"));
        missing_library(&text);
        die!("tlmgr failed (full log: {})", log.display());
    }
}

// fmtutil buries loader errors in tlmgr's output, before any TeX log is written.
fn missing_library(output: &str) {
    let libs: BTreeSet<_> = output.lines().filter(|l| l.contains("error while loading shared libraries")).collect();
    if !libs.is_empty() {
        for l in libs {
            eprintln!("{l}");
        }
        die!("a TeX Live program cannot start; install the missing system library with your package manager and rerun");
    }
}

fn local_tlpdb(tl: &Path) -> String {
    read(&tl.join("tlpkg/texlive.tlpdb")).unwrap_or_else(|| die!("{} is not a TeX Live installation", tl.display()))
}

fn repo_of(local: &str) -> &str {
    local.lines().find_map(|l| l.strip_prefix("depend opt_location:")).unwrap_or_else(|| die!("no opt_location in tlpdb"))
}

fn ensure_tl(year: u32) -> PathBuf {
    let tl = home().join("tl").join(year.to_string());
    if bin(&tl).join("tlmgr").exists() {
        return ready(tl);
    }
    let frozen = format!("{HISTORIC}/{year}/tlnet-final");
    let probe = format!("{frozen}/tlpkg/texlive.tlpdb.xz");
    let repo = if ok(Command::new("curl").args(["-sfIL", "-o", "/dev/null", &probe])) { frozen } else { ROLLING.to_string() };
    println!("texup: installing minimal TeX Live {year} → {}", tl.display());
    let tmp = home().join(format!("tmp-{}", std::process::id()));
    let part = tl.with_extension("partial"); // An interrupted install must not look usable.
    let _ = fs::remove_dir_all(&part);
    fs::create_dir_all(&tmp).unwrap_or_else(|e| die!("{e}"));
    let tgz = tmp.join("install-tl.tar.gz");
    check(Command::new("curl").args(["-sSfL", "-o"]).arg(&tgz).arg(format!("{repo}/install-tl-unx.tar.gz")));
    check(Command::new("tar").arg("xzf").arg(&tgz).arg("-C").arg(&tmp));
    let inst = fs::read_dir(&tmp)
        .unwrap_or_else(|e| die!("{e}"))
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("install-tl-"))
        .unwrap_or_else(|| die!("unexpected install-tl archive layout"))
        .path();
    let d = part.display();
    let profile = format!(
        "selected_scheme scheme-infraonly\nTEXDIR {d}\nTEXMFLOCAL {d}/texmf-local\nTEXMFSYSVAR {d}/texmf-var\n\
         TEXMFSYSCONFIG {d}/texmf-config\nTEXMFVAR {d}/home-var\nTEXMFCONFIG {d}/home-config\nTEXMFHOME {d}/home\n\
         binary_{PLATFORM} 1\ninstopt_adjustpath 0\ntlpdbopt_install_docfiles 0\ntlpdbopt_install_srcfiles 0\n\
         tlpdbopt_autobackup 0\n"
    );
    fs::write(tmp.join("profile"), profile).unwrap_or_else(|e| die!("{e}"));
    check(
        Command::new("perl")
            .arg(inst.join("install-tl"))
            .arg(format!("--profile={}", tmp.join("profile").display()))
            .arg(format!("--repository={repo}"))
            .env("TEXLIVE_INSTALL_ENV_NOCHECK", "1")
            .env("TEXLIVE_INSTALL_NO_WELCOME", "1")
            .stdout(Stdio::null()),
    );
    let _ = fs::remove_dir_all(&tmp);
    if !local_tlpdb(&part).contains(&format!("depend release/{year}\n")) {
        die!("{repo} does not serve TeX Live {year} (installation left at {})", part.display());
    }
    fs::rename(&part, &tl).unwrap_or_else(|e| die!("{e}"));
    ready(tl)
}

fn remote_tlpdb(year: u32, tl: &Path) -> Vec<tlpdb::Pkg> {
    let dir = home().join("tlpdb");
    let path = dir.join(format!("{year}.tlpdb"));
    if !path.exists() {
        fs::create_dir_all(&dir).unwrap_or_else(|e| die!("{e}"));
        let xz = path.with_extension("tlpdb.xz");
        let url = format!("{}/tlpkg/texlive.tlpdb.xz", repo_of(&local_tlpdb(tl)));
        check(Command::new("curl").args(["-sSfL", "-o"]).arg(&xz).arg(url));
        check(Command::new("xz").arg("-df").arg(&xz));
    }
    // shortcut: rolling indexes stay cached; delete this file when package lookups become stale.
    tlpdb::parse(&read(&path).unwrap_or_default())
}

fn installed(tl: &Path) -> HashMap<String, u64> {
    tlpdb::parse(&local_tlpdb(tl)).into_iter().map(|p| (p.name, p.revision)).collect()
}

fn main_of(dir: &Path) -> Option<String> {
    if dir.join("main.tex").exists() {
        return Some("main.tex".into());
    }
    let mut c: Vec<String> = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tex"))
        .filter(|n| {
            read(&dir.join(n)).is_some_and(|t| t.lines().any(|l| l.split('%').next().unwrap_or("").contains("\\documentclass")))
        })
        .collect();
    (c.len() == 1).then(|| c.remove(0))
}

fn manifest(dir: &Path) -> Manifest {
    let text = read(&dir.join("texup.toml")).unwrap_or_else(|| die!("no texup.toml in {}; run texup init first", dir.display()));
    toml::from_str(&text).unwrap_or_else(|e| die!("texup.toml: {e}"))
}

fn setup(dir: &str) -> Project {
    let dir = fs::canonicalize(dir).unwrap_or_else(|e| die!("{dir}: {e}"));
    let m = manifest(&dir);
    // scheme-infraonly has no xelatex; install it up front like latex-bin.
    let (engine, extra) = match m.build.engine.as_deref().unwrap_or("pdflatex") {
        "pdflatex" => ("pdflatex", None),
        "xelatex" => ("xelatex", Some("xetex")),
        "lualatex" => ("lualatex", None),
        e => die!("unsupported engine {e} in texup.toml (use pdflatex, xelatex, or lualatex)"),
    };
    let base = BASE.into_iter().chain(extra).collect();
    let main = m.build.main.or_else(|| main_of(&dir)).unwrap_or_else(|| {
        die!("{}: no unique main file; name it main.tex or set [build] main in texup.toml", dir.display())
    });
    Project { engine, base, tl: ensure_tl(m.texlive.year), year: m.texlive.year, dir, main }
}

fn read_lock(p: &Project) -> Lock {
    read(&p.dir.join("texup.lock"))
        .map(|t| toml::from_str(&t).unwrap_or_else(|e| die!("texup.lock: {e}")))
        .unwrap_or_default()
}

fn fetch(p: &Project) {
    let lock = read_lock(p);
    let mut have = installed(&p.tl);
    let need: BTreeSet<&str> =
        p.base.iter().copied().chain(lock.package.iter().map(|l| l.name.as_str())).filter(|n| !have.contains_key(*n)).collect();
    if !need.is_empty() {
        println!("texup: installing {} packages from lockfile", need.len());
        tlmgr_install(&p.tl, need);
        have = installed(&p.tl);
    }
    for l in &lock.package {
        if let Some(r) = have.get(&l.name).filter(|r| **r != l.revision) {
            eprintln!("texup: warning: {} is r{} in lockfile but r{r} is installed ({} is not fully reproducible)", l.name, l.revision, lock.texlive);
        }
    }
}

/// texweft run in the project directory with outputs next to the sources (`--outdir .`), as
/// LaTeX Workshop's SyncTeX and viewer settings and `why` expect.
fn texweft(p: &Project, cmd: &str) -> Command {
    let mut c = tex(&p.tl, "texweft");
    c.current_dir(&p.dir).args([cmd, &p.main, "--engine", p.engine, "--outdir", ".", "--message-format=json"]);
    c
}

fn texweft_error(e: std::io::Error) -> ! {
    if e.kind() == std::io::ErrorKind::NotFound {
        die!("texweft not found on PATH; install it (see README)");
    }
    die!("texweft: {e}")
}

/// What one texweft build reported: the messages from `build-started` to `build-finished`.
#[derive(Default)]
struct Report {
    /// `build-finished` arrived; without it the other fields describe nothing.
    finished: bool,
    outcome: String,
    preview: Option<String>,
    unsupported: Vec<Value>,
    missing: Vec<Value>,
    errors: Vec<Value>,
    inputs: Vec<(String, String)>,
    programs: Vec<String>,
}

impl Report {
    /// Takes one message; true once the build has finished.
    fn take(&mut self, m: Value) -> bool {
        // texweft comes from PATH at any version; a protocol it does not share must not be misread.
        if m["v"] != 0 {
            die!("texweft speaks protocol v{}; this texup needs v0", m["v"]);
        }
        match m["type"].as_str().unwrap_or("") {
            "build-started" => *self = Report::default(),
            "missing" => self.missing.push(m),
            "unsupported" => self.unsupported.push(m),
            "diagnostic" if m["level"] == "error" => self.errors.push(m),
            "dep-info" => {
                let inputs = m["inputs"].as_array().into_iter().flatten();
                self.inputs = inputs.filter_map(|i| Some((i["path"].as_str()?.into(), i["via"].as_str()?.into()))).collect();
                let programs = m["programs"].as_array().into_iter().flatten();
                self.programs = programs.filter_map(|p| p.as_str().map(String::from)).collect();
            }
            "build-finished" => {
                self.outcome = m["outcome"].as_str().unwrap_or("").into();
                self.preview = m["preview"].as_str().map(String::from);
                self.finished = true;
                return true;
            }
            _ => {}
        }
        false
    }
}

/// Packages that provide what the build reported missing and were not tried yet. Fatal
/// entries end the build; a substituted outline font (PK bitmap, Type 3 in the PDF) is a
/// silent fallback worth fixing too. A shipped `.bbl` standing in for a `.bib` is not.
fn to_install(p: &Project, remote: &mut Option<Vec<tlpdb::Pkg>>, r: &Report, tried: &mut HashSet<String>, locked: bool) -> Vec<String> {
    let wanted: Vec<&Value> = r.missing.iter().filter(|m| m["fatal"] == true || m["kind"] == "font-outline").collect();
    if let Some(lib) = wanted.iter().find(|m| m["kind"] == "library") {
        for l in lib["raw"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            eprintln!("{l}");
        }
        die!("a TeX Live program cannot start; install the missing system library with your package manager and rerun");
    }
    if wanted.is_empty() {
        return Vec::new();
    }
    let db = remote.get_or_insert_with(|| remote_tlpdb(p.year, &p.tl));
    let mut new = Vec::new();
    for m in wanted {
        let name = m["name"].as_str().unwrap_or("");
        let (pkg, what) = match m["kind"].as_str().unwrap_or("") {
            "font-name" => (tlpdb::font_provider(db, name), format!("font \"{name}\"")),
            // texweft reports the TeX font name; its Type 1 file is name.pfb, or a cm-super alias.
            "font-outline" => (tlpdb::providers(db, &format!("{name}.pfb")).first().copied(), format!("outline font {name}")),
            "program" => (tlpdb::bin_provider(db, name, PLATFORM), format!("program {name}")),
            _ => (tlpdb::providers(db, name).first().copied(), name.to_string()),
        };
        match pkg {
            // A METAFONT-only font has no Type 1 file anywhere; the bitmap is the best TeX Live has.
            None if m["kind"] == "font-outline" && m["fatal"] == false => {
                eprintln!("texup: {what} has no Type 1 version in TeX Live {}; the PDF uses a bitmap (Type 3) font", p.year)
            }
            None => eprintln!("texup: no TeX Live {} package provides {what} (missing from the project?)", p.year),
            Some(n) if tried.insert(n.to_string()) => {
                if locked {
                    die!("missing {what} needs {n}, which texup.lock lacks; run texup build and commit texup.lock");
                }
                println!("texup: missing {what} → installing {n}");
                new.push(n.to_string());
            }
            Some(_) => {}
        }
    }
    new
}

/// Locks the packages of every TeX Live file the build read (fonts included, via the `.xdv` or the
/// log), plus those providing the programs it needs: dep-info lists what BibTeX or Biber read, not
/// the binaries, and an on-demand `biber` must be locked for `--locked` on a fresh cache. An
/// up-to-date build reports the same lists, so a deleted lockfile is rewritten without rebuilding.
fn write_lock(p: &Project, r: &Report, locked: bool) {
    if r.inputs.is_empty() {
        return;
    }
    let local = local_tlpdb(&p.tl);
    let db = tlpdb::parse(&local);
    let prefix = format!("{}/", p.tl.display());
    for (f, via) in &r.inputs {
        // Only fonts: a project's own .bib or images are expected to come from the project.
        if (via == "xdv" || via == "log") && !f.starts_with(&prefix) {
            eprintln!("texup: note: {f} is not from TeX Live; Overleaf must provide it too");
        }
    }
    let mut pkgs = tlpdb::owners(&db, r.inputs.iter().filter_map(|(f, _)| f.find("/texmf-dist/").map(|i| &f[i + 1..])));
    for d in db.iter().filter(|d| p.base.contains(&d.name.as_str())) {
        pkgs.insert(&d.name, d.revision);
    }
    for tool in &r.programs {
        if let Some(d) = tlpdb::bin_provider(&db, tool, PLATFORM).and_then(|n| db.iter().find(|d| d.name == n)) {
            pkgs.insert(&d.name, d.revision);
        }
    }
    let kind = if repo_of(&local).ends_with("tlnet-final") { "tlnet-final" } else { "tlnet" };
    let lock = Lock {
        version: 1,
        texlive: format!("{}/{kind}", p.year),
        package: pkgs.into_iter().map(|(n, r)| LockPkg { name: n.into(), revision: r }).collect(),
    };
    let s = toml::to_string(&lock).unwrap_or_else(|e| die!("{e}"));
    let path = p.dir.join("texup.lock");
    if read(&path).as_deref() != Some(s.as_str()) {
        if locked {
            die!("texup.lock is out of date but --locked was passed; run texup build and commit texup.lock");
        }
        fs::write(&path, s).unwrap_or_else(|e| die!("{e}"));
    }
}

/// The preview, whatever the outcome (Overleaf shows a PDF despite errors), then the errors.
fn report(p: &Project, r: &Report) {
    if let Some(pdf) = &r.preview {
        println!("texup: PDF → {pdf}");
    }
    if r.outcome == "converged" {
        return;
    }
    for u in &r.unsupported {
        eprintln!("texup: texweft does not support {} yet ({})", u["flow"].as_str().unwrap_or("?"), u["evidence"].as_str().unwrap_or(""));
    }
    let overleaf = if r.preview.is_some() { " (Overleaf would still produce a PDF)" } else { "" };
    let colon = if r.errors.is_empty() { "" } else { "; errors:" };
    eprintln!("texup: build {}{overleaf}{colon}", r.outcome);
    let line = |e: &Value| {
        let file = e["span"]["file"].as_str().map(|f| Path::new(f).strip_prefix(&p.dir).map_or(f.into(), |r| r.display().to_string()));
        match (file, e["span"]["line"].as_u64()) {
            (Some(f), Some(n)) => format!("{f}:{n}: {}", e["message"].as_str().unwrap_or("")),
            _ => e["message"].as_str().unwrap_or("").to_string(),
        }
    };
    for l in r.errors.iter().map(line).collect::<BTreeSet<_>>().into_iter().take(20) {
        eprintln!("{l}");
    }
}

fn messages(out: &[u8]) -> impl Iterator<Item = Value> + '_ {
    out.split(|b| *b == b'\n').filter(|l| !l.is_empty()).filter_map(|l| serde_json::from_slice(l).ok())
}

fn build(p: &Project, locked: bool) -> i32 {
    if locked && !p.dir.join("texup.lock").exists() {
        die!("--locked requires texup.lock; run texup build and commit it");
    }
    fetch(p);
    let mut remote = None;
    let mut tried = HashSet::new();
    // Each install changes what texweft fingerprints (ls-R, maps, the inputs themselves), so a
    // retry needs no force flag. Never retrying a package bounds the loop; it is not capped.
    let r = loop {
        let out = texweft(p, "build").stderr(Stdio::inherit()).output().unwrap_or_else(|e| texweft_error(e));
        let mut r = Report::default();
        for m in messages(&out.stdout) {
            r.take(m);
        }
        // A crash, or an older texweft rejecting an option (usage, exit 2), reports no result.
        if !r.finished {
            die!("texweft exited without a result (status {})", out.status.code().map_or("killed".into(), |c| c.to_string()));
        }
        let new = to_install(p, &mut remote, &r, &mut tried, locked);
        if new.is_empty() {
            break r;
        }
        tlmgr_install(&p.tl, new);
    };
    write_lock(p, &r, locked);
    report(p, &r);
    if r.outcome == "converged" { 0 } else { 1 }
}

/// Builds on every change, installing what a build reports missing. texweft watches ls-R, which
/// tlmgr rewrites on install, so it rebuilds by itself once the package is in.
fn watch(p: &Project) -> ! {
    fetch(p);
    let mut child = texweft(p, "watch").stdout(Stdio::piped()).spawn().unwrap_or_else(|e| texweft_error(e));
    let mut remote = None;
    let mut tried = HashSet::new();
    let mut r = Report::default();
    for line in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) {
        let Ok(m) = serde_json::from_str(&line) else { continue };
        if !r.take(m) {
            continue;
        }
        let new = to_install(p, &mut remote, &r, &mut tried, false);
        if new.is_empty() {
            write_lock(p, &r, false);
            report(p, &r);
        } else {
            tlmgr_install(&p.tl, new);
        }
    }
    let status = child.wait().ok().and_then(|s| s.code());
    if !r.finished && status != Some(0) {
        die!("texweft exited without a result (status {})", status.map_or("killed".into(), |c| c.to_string()));
    }
    exit(status.unwrap_or(1))
}

fn why(p: &Project, pkg: &str) {
    if p.base.contains(&pkg) {
        println!("texup: {pkg} is always locked (needed to compile)");
        return;
    }
    let fls = read(&p.file("fls")).unwrap_or_else(|| die!("no {}; run texup build first", p.file("fls").display()));
    let db = tlpdb::parse(&local_tlpdb(&p.tl));
    let d = db.iter().find(|d| d.name == pkg).unwrap_or_else(|| die!("{pkg} is not installed"));
    let files: BTreeSet<_> = scan::tl_inputs(&fls).filter(|f| d.runfiles.iter().any(|r| r == f)).collect();
    if files.is_empty() {
        println!("texup: the last build read no files from {pkg}");
    }
    for f in files {
        println!("{f}");
    }
}

fn exec(args: &[String]) -> ! {
    let Some(cmd) = args.first() else { die!("usage: texup exec <command> [args...]") };
    let year = manifest(Path::new(".")).texlive.year;
    let tl = ensure_tl(year);
    let bin = bin(&tl).join(cmd);
    if !bin.exists() {
        let db = remote_tlpdb(year, &tl);
        let pkg = tlpdb::bin_provider(&db, cmd, PLATFORM).unwrap_or_else(|| die!("no TeX Live {year} package provides the command {cmd}"));
        println!("texup: installing {pkg} for {cmd}");
        tlmgr_install(&tl, [pkg]);
    }
    let e = tex(&tl, bin).args(&args[1..]).exec();
    die!("{cmd}: {e}")
}

fn log_year(dir: &Path) -> Option<u32> {
    fs::read_dir(dir).ok()?.filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "log")).find_map(|e| {
        let first = read(&e.path())?.lines().next()?.to_string();
        first.split("(TeX Live ").nth(1)?.get(..4)?.parse().ok()
    })
}

fn init(dir: &str, year: Option<u32>) {
    let d = Path::new(dir);
    let path = d.join("texup.toml");
    if path.exists() {
        die!("{} already exists", path.display());
    }
    let year = year
        .or_else(|| log_year(d))
        .unwrap_or_else(|| die!("cannot infer TeX Live year; pass --year (match the version selected in Overleaf)"));
    let main = main_of(d).map(|m| format!("main = \"{m}\"\n")).unwrap_or_default();
    let s = format!("[texlive]\nyear = {year}\n\n[build]\n{main}engine = \"pdflatex\"\n");
    fs::write(&path, &s).unwrap_or_else(|e| die!("{e}"));
    print!("texup: created {}\n{s}", path.display());
}

const HELP: &str = "\
texup init  [dir] [--year Y]  create texup.toml (year inferred from an existing .log)
texup build [dir] [--locked]  compile, installing missing packages and rerunning; writes texup.lock
                              (--locked: fail instead of changing texup.lock, e.g. in CI)
texup watch [dir]             build, then rebuild on every change (no viewer is opened)
texup fetch [dir]             only install packages from texup.lock (e.g. before going offline)
texup clean [dir]             remove build outputs (texweft clean)
texup why <package> [dir]     list the files from <package> the last build read
texup exec <cmd> [args...]    run a TeX Live command of this project's year, installing it if needed
texup --version               print the texup version
cache: ${TEXUP_HOME:-~/.cache/texup}";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map_or("", String::as_str);
    if cmd == "exec" {
        exec(&args[1..]);
    }
    let (mut pos, mut year, mut locked) = (Vec::new(), None, false);
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--year" => year = Some(it.next().and_then(|y| y.parse().ok()).unwrap_or_else(|| die!("--year requires a year"))),
            "--locked" => locked = true,
            _ => pos.push(a.as_str()),
        }
    }
    let dir = pos.first().copied().unwrap_or(".");
    match cmd {
        "--version" | "-V" => println!("texup {}", env!("CARGO_PKG_VERSION")),
        "init" => init(dir, year),
        "build" => exit(build(&setup(dir), locked)),
        "fetch" => fetch(&setup(dir)),
        "why" => {
            let pkg = pos.first().unwrap_or_else(|| die!("usage: texup why <package> [dir]"));
            why(&setup(pos.get(1).copied().unwrap_or(".")), pkg);
        }
        "watch" => watch(&setup(dir)),
        "clean" => {
            let p = setup(dir);
            if !texweft(&p, "clean").status().unwrap_or_else(|e| texweft_error(e)).success() {
                exit(1);
            }
        }
        _ => println!("{HELP}"),
    }
}
