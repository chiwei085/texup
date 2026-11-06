use std::collections::{BTreeMap, HashMap};

pub struct Pkg {
    pub name: String,
    pub revision: u64,
    pub runfiles: Vec<String>,
    pub binfiles: Vec<String>,
}

pub fn parse(text: &str) -> Vec<Pkg> {
    let mut out = Vec::new();
    for rec in text.split("\n\n") {
        let mut p = Pkg { name: String::new(), revision: 0, runfiles: Vec::new(), binfiles: Vec::new() };
        let mut section = "";
        for line in rec.lines() {
            if let Some(f) = line.strip_prefix(' ') {
                match section {
                    "runfiles" => p.runfiles.push(match f.strip_prefix("RELOC/") {
                        Some(r) => format!("texmf-dist/{r}"),
                        None => f.to_string(),
                    }),
                    "binfiles" => p.binfiles.push(f.to_string()),
                    _ => {}
                }
                continue;
            }
            section = line.split(' ').next().unwrap_or("");
            if let Some(v) = line.strip_prefix("name ") {
                p.name = v.to_string();
            } else if let Some(v) = line.strip_prefix("revision ") {
                p.revision = v.parse().unwrap_or(0);
            }
        }
        if !p.name.is_empty() {
            out.push(p);
        }
    }
    out
}

pub fn providers<'a>(pkgs: &'a [Pkg], file: &str) -> Vec<&'a str> {
    let suffix = format!("/{file}");
    let (mut v, mut dev): (Vec<&str>, Vec<&str>) = pkgs
        .iter()
        .filter(|p| p.runfiles.iter().any(|f| f.ends_with(&suffix)))
        .map(|p| p.name.as_str())
        .partition(|n| !n.ends_with("-dev"));
    v.append(&mut dev);
    // shortcut: EC/TC font aliases are absent from tlpdb; parse .map files when other aliases are needed.
    if v.is_empty() && file.ends_with(".pfb") && (file.starts_with("ec") || file.starts_with("tc")) {
        v.push("cm-super");
    }
    v
}

// shortcut: tlpdb has no font names, so match "TeX Gyre Termes" against the file name stem
// of texgyretermes-regular.otf; a name unlike its files (Latin Modern → lmroman10) is not found.
pub fn font_provider<'a>(pkgs: &'a [Pkg], name: &str) -> Option<&'a str> {
    let norm = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
    let want = norm(name);
    let hit = |f: &String| {
        let stem = f.rsplit('/').next().unwrap_or("").split(['-', '.', '_']).next().unwrap_or("");
        (f.contains("/fonts/opentype/") || f.contains("/fonts/truetype/")) && norm(stem) == want
    };
    let mut v: Vec<&str> = pkgs.iter().filter(|p| p.runfiles.iter().any(hit)).map(|p| p.name.as_str()).collect();
    v.sort_by_key(|n| n.ends_with("-dev"));
    v.first().copied()
}

// Executables live in `<pkg>.x86_64-linux`; installing `<pkg>` pulls in both halves.
pub fn bin_provider<'a>(pkgs: &'a [Pkg], cmd: &str) -> Option<&'a str> {
    let f = format!("bin/x86_64-linux/{cmd}");
    let p = pkgs.iter().find(|p| p.binfiles.contains(&f))?;
    Some(p.name.strip_suffix(".x86_64-linux").unwrap_or(&p.name))
}

pub fn owners<'a, 'b>(pkgs: &'a [Pkg], paths: impl Iterator<Item = &'b str>) -> BTreeMap<&'a str, u64> {
    let by_file: HashMap<&str, &Pkg> =
        pkgs.iter().flat_map(|p| p.runfiles.iter().map(move |f| (f.as_str(), p))).collect();
    paths.filter_map(|f| by_file.get(f)).map(|p| (p.name.as_str(), p.revision)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Pkg> {
        parse(include_str!("../tests/fixtures/remote.tlpdb"))
    }

    #[test]
    fn parses_records() {
        let db = fixture();
        let tools = db.iter().find(|p| p.name == "tools").unwrap();
        assert_eq!(tools.revision, 68941);
        assert!(tools.runfiles.contains(&"texmf-dist/tex/latex/tools/array.sty".to_string()));
        assert!(!tools.runfiles.iter().any(|f| f.contains("/source/") || f.contains("/doc/")));
    }

    #[test]
    fn dev_sorts_last() {
        let db = fixture();
        assert_eq!(providers(&db, "array.sty"), ["tools", "latex-tools-dev"]);
        assert_eq!(providers(&db, "tcrm1200.tfm"), ["ec"]);
        assert_eq!(providers(&db, "cmr12.pfb"), ["amsfonts"]);
        assert!(providers(&db, "nope.sty").is_empty());
        assert_eq!(providers(&db, "tcrm1200.pfb"), ["cm-super"]);
    }

    #[test]
    fn font_name_to_package() {
        let db = fixture();
        assert_eq!(font_provider(&db, "TeX Gyre Termes"), Some("tex-gyre"));
        assert_eq!(font_provider(&db, "Noto Serif CJK TC"), None);
    }

    #[test]
    fn command_to_package() {
        let db = fixture();
        assert_eq!(bin_provider(&db, "latexdiff-vc"), Some("latexdiff"));
        assert_eq!(bin_provider(&db, "latexdiff-v"), None);
        assert!(db.iter().all(|p| p.binfiles.is_empty() || p.runfiles.is_empty()));
    }

    #[test]
    fn owners_by_full_path() {
        let db = fixture();
        let o = owners(&db, ["texmf-dist/tex/latex-dev/tools/array.sty", "texmf-dist/x/unknown"].into_iter());
        assert_eq!(o.into_iter().collect::<Vec<_>>(), [("latex-tools-dev", 69872)]);
    }
}
