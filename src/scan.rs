use std::collections::BTreeSet;

fn is_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

// `Font \x=<name> at 10pt not loadable: ...` → `<name>`. pdfTeX and XeTeX say
// `Metric (TFM) file ... not found`, LuaTeX `metric data not found or bad`.
fn unloadable(line: &str) -> Option<&str> {
    let head = &line[..line.find(" not loadable: ")?];
    Some(head.split(" at ").next().unwrap_or(head).split_once('=')?.1)
}

// Fonts requested by name, which tlpdb cannot resolve directly: XeTeX writes
// `="TeX Gyre Termes"`, LuaTeX `=name:TeX Gyre Termes:mode=node;...`.
pub fn missing_fonts(log: &str) -> BTreeSet<String> {
    log.lines()
        .filter_map(unloadable)
        .filter_map(|n| n.strip_prefix('"').and_then(|n| n.strip_suffix('"')).or_else(|| n.strip_prefix("name:")))
        .filter_map(|n| n.split([':', '/']).next()) // drop `/OT` or `:features`
        .map(String::from)
        .collect()
}

// Font files an XDV file uses: each native font definition (opcode 252) is
// k[4] size[4] flags[2] len[1] path[len], with an absolute path.
pub fn xdv_fonts(xdv: &[u8]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (i, _) in xdv.iter().enumerate().filter(|(_, b)| **b == 252) {
        let Some(&len) = xdv.get(i + 11) else { continue };
        if let Some(Ok(p)) = xdv.get(i + 12..i + 12 + len as usize).map(std::str::from_utf8)
            && p.starts_with('/')
            && p.contains("/fonts/")
        {
            out.insert(p.to_string());
        }
    }
    out
}

// Font files pdfTeX and LuaTeX embedded, listed as `</abs/path.otf>` before `Output written`.
pub fn log_fonts(log: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for part in log.lines().filter(|l| l.starts_with("</")).flat_map(|l| l.split('<')) {
        if let Some(p) = part.split('>').next().filter(|p| p.starts_with('/') && p.contains("/fonts/")) {
            out.insert(p.to_string());
        }
    }
    out
}

pub fn missing(log: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in log.lines() {
        for (pre, post) in [("File `", "' not found"), ("I can't find file `", "'")] {
            if let Some(rest) = line.find(pre).map(|i| &line[i + pre.len()..])
                && let Some(j) = rest.find('\'')
                && rest[j..].starts_with(post)
            {
                let f = &rest[..j];
                out.insert(if f.contains('.') { f.to_string() } else { format!("{f}.tex") });
            }
        }
        if let Some(name) = unloadable(line) {
            if let Some(f) = name.strip_prefix('[').and_then(|n| n.split_once(']')).map(|(f, _)| f) {
                out.insert(f.to_string()); // XeTeX font loaded by file name
            } else if !name.is_empty() && name.chars().all(is_name) {
                out.insert(format!("{name}.tfm"));
            }
        }
        if let Some(rest) = line.split_once("): Font ").map(|(_, r)| r)
            && let Some((name, _)) = rest.split_once(" at ")
            && line.ends_with(" not found")
        {
            out.insert(format!("{name}.pfb"));
        }
        for (i, _) in line.match_indices("pk>") {
            let s = line[..i].trim_end_matches(|c: char| c.is_ascii_digit());
            if let Some(s) = s.strip_suffix('.') {
                let name = &s[s.trim_end_matches(is_name).len()..];
                if !name.is_empty() && s[..s.len() - name.len()].ends_with('/') {
                    out.insert(format!("{name}.pfb"));
                }
            }
        }
    }
    out
}

pub fn tl_inputs(fls: &str) -> impl Iterator<Item = &str> {
    fls.lines()
        .filter_map(|l| l.strip_prefix("INPUT /"))
        .filter_map(|p| p.find("/texmf-dist/").map(|i| &p[i + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tlpdb;

    #[test]
    fn missing_from_real_log() {
        let got = missing(include_str!("../tests/fixtures/missing.log"));
        let want = [
            "cmr12.pfb",
            "tcrm1200.pfb",
            "texgyrepagella-regular.otf",
            "texgyretermes-regular.otf",
            "zzfont.tfm",
            "zzfontb.tfm",
            "zzmissing.sty",
            "zznofile.tex",
        ];
        assert_eq!(got.iter().map(String::as_str).collect::<Vec<_>>(), want);
        let fonts = missing_fonts(include_str!("../tests/fixtures/missing.log"));
        assert_eq!(fonts.into_iter().collect::<Vec<_>>(), ["TeX Gyre Termes"]);
        let lua: String = include_str!("../tests/fixtures/missing.log").lines().filter(|l| l.contains("=name:")).collect();
        assert_eq!(missing_fonts(&lua).into_iter().collect::<Vec<_>>(), ["TeX Gyre Termes"]);
    }

    #[test]
    fn fonts_from_lualatex_log() {
        let got = log_fonts(include_str!("../tests/fixtures/lualatex-tail.log"));
        let names: Vec<_> = got.iter().map(|p| p.rsplit('/').next().unwrap()).collect();
        assert_eq!(names, ["lmroman10-regular.otf", "NotoSerifCJK-Regular.ttc"]);
    }

    #[test]
    fn fonts_from_xdv() {
        let got = xdv_fonts(include_bytes!("../tests/fixtures/xecjk.xdv"));
        let names: Vec<_> = got.iter().map(|p| p.rsplit('/').next().unwrap()).collect();
        assert_eq!(names, ["FandolSong-Regular.otf", "lmroman10-regular.otf"]);
        assert!(got.iter().all(|p| p.contains("/texmf-dist/fonts/opentype/")));
    }

    #[test]
    fn fls_to_packages() {
        let fls = include_str!("../tests/fixtures/demo.fls");
        let db = tlpdb::parse(include_str!("../tests/fixtures/demo-local.tlpdb"));
        let got: Vec<_> = tlpdb::owners(&db, tl_inputs(fls)).into_keys().collect();
        assert_eq!(got, ["amsfonts", "arphic", "cjk", "cm", "iftex", "kpathsea", "l3backend", "latex"]);
        assert!(tl_inputs(fls).all(|p| p.starts_with("texmf-dist/")));
    }
}
