use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = Self(std::env::temp_dir().join(format!("texup-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))));
        fs::create_dir_all(p.0.join("paper")).unwrap();
        fs::create_dir_all(p.0.join("tools")).unwrap();
        #[cfg(target_arch = "x86_64")]
        let platform = "x86_64-linux";
        #[cfg(target_arch = "aarch64")]
        let platform = "aarch64-linux";
        let bin = p.0.join(format!("cache/tl/2025/bin/{platform}"));
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(p.0.join("cache/tl/2025/tlpkg")).unwrap();
        fs::create_dir_all(p.0.join("cache/tlpdb")).unwrap();
        fs::write(p.0.join("texup.toml"), "[texlive]\nyear = 2025\n[build]\nmain = \"paper/main.tex\"\n").unwrap();
        fs::write(p.0.join("paper/main.tex"), "\\documentclass{article}").unwrap();
        fs::write(p.0.join("cache/tl/2025/tlpkg/texlive.tlpdb"), "name 00texlive.installation\ndepend opt_location:https://example.invalid/tlnet-final\n\nname latex-bin\nrevision 1\n\nname bibtex\nrevision 2\n").unwrap();
        fs::write(p.0.join("cache/tlpdb/2025.tlpdb"), "name tools\nrevision 3\nrunfiles size=1\n RELOC/tex/latex/tools/array.sty\n").unwrap();
        p.script(bin.join("tlmgr"), "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$TEXUP_HOME/installs\"\ncat >> \"$TEXUP_HOME/tl/2025/tlpkg/texlive.tlpdb\" <<'DB'\n\nname tools\nrevision 3\nrunfiles size=1\n texmf-dist/tex/latex/tools/array.sty\nDB\n");
        p.script(p.0.join("tools/texweft"), r##"#!/bin/sh
printf '%s\n' "$*" >> "$TEXUP_HOME/commands"
printf '%s\n' '{"v":0,"type":"build-started"}'
if [ "$TEXUP_TEST_MISSING" = 1 ] && [ ! -f "$TEXUP_HOME/installs" ]; then
    printf '%s\n' '{"v":0,"type":"missing","kind":"file","name":"array.sty","fatal":true}' '{"v":0,"type":"build-finished","outcome":"missing","preview":null}'
    exit 1
fi
printf '{"v":0,"type":"dep-info","inputs":[{"path":"%s/tl/2025/texmf-dist/tex/latex/tools/array.sty","via":"fls"}],"programs":[]}\n' "$TEXUP_HOME"
printf '{"v":0,"type":"build-finished","outcome":"%s","preview":"paper/main.pdf"}\n' "${TEXUP_TEST_OUTCOME:-converged}"
"##);
        p
    }

    fn script(&self, path: PathBuf, text: &str) {
        fs::write(&path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_texup"));
        c.current_dir(&self.0).env("TEXUP_HOME", self.0.join("cache"));
        c.env("PATH", std::env::join_paths(std::iter::once(self.0.join("tools")).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn installs_missing_package_and_locks_the_main_files_directory() {
    let p = Project::new();
    let out = p.command().arg("build").env("TEXUP_TEST_MISSING", "1").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(fs::read_to_string(p.0.join("cache/installs")).unwrap(), "install tools\n");
    assert!(p.0.join("paper/.main.texweft.lock").exists());
    assert!(!p.0.join(".main.texweft.lock").exists());
    let lock = fs::read_to_string(p.0.join("texup.lock")).unwrap();
    assert!(lock.contains("name = \"tools\"\nrevision = 3"));
    let modified = fs::metadata(p.0.join("texup.lock")).unwrap().modified().unwrap();
    assert!(p.run(&["build", "--locked"]).status.success());
    assert_eq!(fs::metadata(p.0.join("texup.lock")).unwrap().modified().unwrap(), modified);
    assert_eq!(fs::read_to_string(p.0.join("cache/commands")).unwrap().lines().count(), 3);
}

#[test]
fn locked_build_refuses_new_packages_and_missing_lockfile() {
    let p = Project::new();
    let out = p.run(&["build", "--locked"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--locked requires texup.lock"));
    assert!(!p.0.join("cache/commands").exists());
    assert!(p.run(&["build"]).status.success());
    let lock = fs::read(p.0.join("texup.lock")).unwrap();
    let out = p.command().args(["build", "--locked"]).env("TEXUP_TEST_MISSING", "1").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("needs tools"));
    assert!(!p.0.join("cache/installs").exists());
    assert_eq!(fs::read(p.0.join("texup.lock")).unwrap(), lock);
}

#[test]
fn failed_build_keeps_preview_but_exits_nonzero() {
    let p = Project::new();
    let out = p.command().arg("build").env("TEXUP_TEST_OUTCOME", "failed").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("PDF → paper/main.pdf"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("build failed"));
}

#[test]
fn rejects_protocol_mismatch_and_missing_result() {
    let p = Project::new();
    for (script, error) in [
        ("#!/bin/sh\necho '{\"v\":1,\"type\":\"build-finished\",\"outcome\":\"converged\"}'\n", "needs v0"),
        ("#!/bin/sh\nexit 2\n", "without a result (status 2)"),
    ] {
        p.script(p.0.join("tools/texweft"), script);
        let out = p.run(&["build"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(error));
    }
}

#[test]
fn never_retries_the_same_package() {
    let p = Project::new();
    p.script(p.0.join("tools/texweft"), "#!/bin/sh\nprintf '%s\\n' '{\"v\":0,\"type\":\"missing\",\"kind\":\"file\",\"name\":\"array.sty\",\"fatal\":true}' '{\"v\":0,\"type\":\"build-finished\",\"outcome\":\"missing\"}'\nexit 1\n");
    let out = p.run(&["build"]);
    assert!(!out.status.success());
    assert_eq!(fs::read_to_string(p.0.join("cache/installs")).unwrap(), "install tools\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("build missing"));
}
