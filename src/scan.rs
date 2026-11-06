// TeX Live files a build read, from its `.fls`, for `texup why`. Missing-file detection and font
// discovery now come from texweft's messages.

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
    fn fls_to_packages() {
        let fls = include_str!("../tests/fixtures/demo.fls");
        let db = tlpdb::parse(include_str!("../tests/fixtures/demo-local.tlpdb"));
        let got: Vec<_> = tlpdb::owners(&db, tl_inputs(fls)).into_keys().collect();
        assert_eq!(got, ["amsfonts", "arphic", "cjk", "cm", "iftex", "kpathsea", "l3backend", "latex"]);
        assert!(tl_inputs(fls).all(|p| p.starts_with("texmf-dist/")));
    }
}
