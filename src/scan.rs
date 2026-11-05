use std::collections::BTreeSet;

fn is_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
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
        if let Some(i) = line.find(" not loadable: Metric (TFM)") {
            let head = &line[..i];
            let head = head.split(" at ").next().unwrap_or(head);
            if let Some((_, name)) = head.rsplit_once('=')
                && !name.is_empty()
                && name.chars().all(is_name)
            {
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
        let want = ["cmr12.pfb", "tcrm1200.pfb", "zzfont.tfm", "zzfontb.tfm", "zzmissing.sty", "zznofile.tex"];
        assert_eq!(got.iter().map(String::as_str).collect::<Vec<_>>(), want);
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
