mod scan;
mod tlpdb;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio, exit};
use std::env;

const HISTORIC: &str = "https://ftp.math.utah.edu/pub/tex/historic/systems/texlive";
const ROLLING: &str = "https://mirror.ctan.org/systems/texlive/tlnet";
const BASE: [&str; 3] = ["latex-bin", "latexmk", "bibtex"];
const LATEXMK: [&str; 5] = ["-pdf", "-f", "-synctex=1", "-interaction=nonstopmode", "-file-line-error"];
const BIN: &str = "bin/x86_64-linux";

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
    c.status().is_ok_and(|s| s.success())
}

fn check(c: &mut Command) {
    if !ok(c) {
        die!("{:?} failed", c.get_program());
    }
}

fn tex(tl: &Path, prog: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(prog);
    let path = env::var("PATH").unwrap_or_default();
    c.env("PATH", format!("{}:{path}", tl.join(BIN).display())).env("max_print_line", "10000");
    c
}

// An absolute path lets tlmgr locate its installation.
fn tlmgr_install<S: AsRef<std::ffi::OsStr>>(tl: &Path, pkgs: impl IntoIterator<Item = S>) {
    let log = home().join("tlmgr.log");
    let f = fs::OpenOptions::new().create(true).append(true).open(&log).unwrap_or_else(|e| die!("{e}"));
    let out = f.try_clone().unwrap_or_else(|e| die!("{e}"));
    if !ok(tex(tl, tl.join(BIN).join("tlmgr")).arg("install").args(pkgs).stdout(out).stderr(f)) {
        let text = read(&log).unwrap_or_default();
        let lines: Vec<_> = text.lines().collect();
        eprintln!("{}", lines[lines.len().saturating_sub(20)..].join("\n"));
        die!("tlmgr failed (full log: {})", log.display());
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
    if tl.join(BIN).join("tlmgr").exists() {
        return fs::canonicalize(&tl).unwrap_or(tl);
    }
    let frozen = format!("{HISTORIC}/{year}/tlnet-final");
    let probe = format!("{frozen}/tlpkg/texlive.tlpdb.xz");
    let repo = if ok(Command::new("curl").args(["-sfIL", "-o", "/dev/null", &probe])) { frozen } else { ROLLING.to_string() };
    println!("texup: installing minimal TeX Live {year} → {}", tl.display());
    let tmp = home().join(format!("tmp-{}", std::process::id()));
    let part = tl.with_extension("partial"); // Keep incomplete installations separate.
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
         binary_x86_64-linux 1\ninstopt_adjustpath 0\ntlpdbopt_install_docfiles 0\ntlpdbopt_install_srcfiles 0\n\
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
    fs::canonicalize(&tl).unwrap_or(tl)
}

fn remote_tlpdb(p: &Project) -> Vec<tlpdb::Pkg> {
    let dir = home().join("tlpdb");
    let path = dir.join(format!("{}.tlpdb", p.year));
    if !path.exists() {
        fs::create_dir_all(&dir).unwrap_or_else(|e| die!("{e}"));
        let xz = path.with_extension("tlpdb.xz");
        let url = format!("{}/tlpkg/texlive.tlpdb.xz", repo_of(&local_tlpdb(&p.tl)));
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

fn setup(dir: &str) -> Project {
    let dir = fs::canonicalize(dir).unwrap_or_else(|e| die!("{dir}: {e}"));
    let text = read(&dir.join("texup.toml")).unwrap_or_else(|| die!("no texup.toml in {}; run texup init first", dir.display()));
    let m: Manifest = toml::from_str(&text).unwrap_or_else(|e| die!("texup.toml: {e}"));
    if let Some(e) = m.build.engine.as_deref().filter(|e| *e != "pdflatex") {
        die!("only pdflatex is supported (texup.toml says {e})");
    }
    let main = m.build.main.or_else(|| main_of(&dir)).unwrap_or_else(|| {
        die!("{}: no unique main file; name it main.tex or set [build] main in texup.toml", dir.display())
    });
    Project { tl: ensure_tl(m.texlive.year), year: m.texlive.year, dir, main }
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
        BASE.into_iter().chain(lock.package.iter().map(|l| l.name.as_str())).filter(|n| !have.contains_key(*n)).collect();
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

fn write_lock(p: &Project) {
    let Some(fls) = read(&p.file("fls")) else { return };
    let local = local_tlpdb(&p.tl);
    let db = tlpdb::parse(&local);
    let mut pkgs = tlpdb::owners(&db, scan::tl_inputs(&fls));
    for d in db.iter().filter(|d| BASE.contains(&d.name.as_str())) {
        pkgs.insert(&d.name, d.revision);
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
        fs::write(&path, s).unwrap_or_else(|e| die!("{e}"));
    }
}

fn is_error_line(l: &str) -> bool {
    l.starts_with("! ")
        || l.split_once(' ').and_then(|(t, _)| t.strip_suffix(':')?.rsplit_once(':')).is_some_and(|(f, n)| {
            !f.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
        })
}

fn build(p: &Project) -> i32 {
    fetch(p);
    let log = p.file("log");
    let mut remote = None;
    let mut tried = HashSet::new();
    let mut rc;
    // latexmk misses TeX Live changes unless forced to rebuild.
    let mine = format!("INPUT {}/", p.tl.display());
    let mut g = read(&p.file("fls")).is_some_and(|f| !f.contains(&mine));
    loop {
        let mut c = tex(&p.tl, "latexmk");
        c.current_dir(&p.dir).args(g.then_some("-g")).args(LATEXMK).arg(&p.main);
        rc = c.stdout(Stdio::null()).stderr(Stdio::null()).status().map_or_else(|e| die!("latexmk: {e}"), |s| s.code().unwrap_or(1));
        let mut new = Vec::new();
        for f in scan::missing(&read(&log).unwrap_or_default()) {
            let db = remote.get_or_insert_with(|| remote_tlpdb(p));
            match tlpdb::providers(db, &f).first() {
                None => eprintln!("texup: no TeX Live {} package provides {f} (missing from the project?)", p.year),
                Some(n) if tried.insert(n.to_string()) => {
                    println!("texup: missing {f} → installing {n}");
                    new.push(n.to_string());
                }
                Some(_) => {}
            }
        }
        if new.is_empty() {
            break;
        }
        tlmgr_install(&p.tl, new);
        g = true; // New packages do not change source timestamps.
    }
    write_lock(p);
    let pdf = p.file("pdf");
    if pdf.exists() {
        println!("texup: PDF → {}", pdf.display());
    }
    if rc != 0 {
        eprintln!("texup: latexmk exited with {rc} (Overleaf would still produce a PDF); errors:");
        let text = read(&log).unwrap_or_default();
        for l in text.lines().filter(|l| is_error_line(l)).collect::<BTreeSet<_>>().into_iter().take(20) {
            eprintln!("{l}");
        }
        let blg = read(&p.file("blg")).unwrap_or_default();
        let bad = |l: &&str| l.contains("error message") || l.starts_with("I couldn") || l.starts_with("Repeated");
        for l in blg.lines().filter(bad).take(5) {
            eprintln!("{l}");
        }
    }
    rc
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
texup build [dir]             compile, installing missing packages and rerunning; writes texup.lock
texup watch [dir]             build, then keep watching with latexmk -pvc
texup fetch [dir]             only install packages from texup.lock (e.g. before going offline)
texup clean [dir]             latexmk -C
cache: ${TEXUP_HOME:-~/.cache/texup}";

fn main() {
    let mut args = env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let (mut dir, mut year) = (".".to_string(), None);
    while let Some(a) = args.next() {
        if a == "--year" {
            year = Some(args.next().and_then(|y| y.parse().ok()).unwrap_or_else(|| die!("--year requires a year")));
        } else {
            dir = a;
        }
    }
    match cmd.as_str() {
        "init" => init(&dir, year),
        "build" => exit(build(&setup(&dir))),
        "fetch" => fetch(&setup(&dir)),
        "watch" => {
            let p = setup(&dir);
            build(&p);
            let e = tex(&p.tl, "latexmk").current_dir(&p.dir).arg("-pvc").args(LATEXMK).arg(&p.main).exec();
            die!("latexmk: {e}");
        }
        "clean" => {
            let p = setup(&dir);
            check(tex(&p.tl, "latexmk").current_dir(&p.dir).arg("-C").arg(&p.main));
        }
        _ => println!("{HELP}"),
    }
}
