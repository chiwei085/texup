# texup — Development guide

See [README.md](README.md) for user documentation. This file records design
principles and verified TeX Live behavior. Read it before changing related logic.

## Code structure

- `src/main.rs`: CLI, manifest and lockfile handling, TeX Live installation
  (`install-tl` / `tlmgr`), and the build loop that installs missing packages.
- `src/tlpdb.rs`: `texlive.tlpdb` parsing; filename-to-provider lookup with
  `*-dev` packages ranked last; full-path-to-package lookup; command-to-package
  lookup for `texup exec`.
- `src/scan.rs`: missing-file detection from `.log` and extraction of TeX Live
  files actually read from `.fls` and fonts used from `.xdv`.
- `tests/fixtures/`: excerpts from real tlpdb, log, `.fls`, and `.xdv` files.

## Principles

- Scope: the manifest/lockfile contract, tlpdb parsing, invoking `install-tl`,
  `tlmgr`, and `latexmk`, installing missing packages, and generating the lockfile
  from `.fls`.
- Out of scope: modifying TeX engines, FUSE or lazy filesystems, and reimplementing
  TeX Live installation semantics. Leave `.fmt` generation and `updmap` to
  `tlmgr`. These alternatives were evaluated and were not worth the complexity.
- Support only x86_64 Linux, pdfLaTeX, and XeLaTeX. The `engine` field is
  reserved for future LuaLaTeX support.
- Keep dependencies limited to `serde` and `toml`. Use `curl`, `tar`, and `xz`
  directly for downloads and extraction.
- **Do not generate `latexmkrc`:** Overleaf reads it.
- Write user-facing messages in English, prefixed with `texup: `.

## Verified TeX Live behavior

Preserve the following handling when changing related code.

### Installation

- Frozen-year repository:
  `https://ftp.math.utah.edu/pub/tex/historic/systems/texlive/<year>/tlnet-final`.
  The current year has no frozen repository (404), so fall back to
  `https://mirror.ctan.org/systems/texlive/tlnet`. Probe with a HEAD request.
  A rolling mirror may already serve a different year; after installation,
  verify that the local tlpdb contains `depend release/<year>`.
- Use this `install-tl` profile: `scheme-infraonly`,
  `tlpdbopt_install_docfiles 0`, `tlpdbopt_install_srcfiles 0`, and
  `instopt_adjustpath 0`. Keep `TEXDIR` and all `TEXMF*` paths inside the
  installation directory. Install into `<year>.partial`, then rename it so an
  incomplete installation cannot be mistaken for a usable one.
- Set both `TEXLIVE_INSTALL_ENV_NOCHECK=1` and `TEXLIVE_INSTALL_NO_WELCOME=1`.
- The installed `texmf.cnf` uses `$SELFAUTOPARENT`, allowing the installation
  directory to be moved directly with `mv`.
- Invoke `tlmgr` by its absolute path; otherwise it reports
  "Could not determine directory of tlmgr executable".
- Frozen-year `tlmgr` prints noisy "TeX Live YYYY is frozen" messages. Redirect
  its output to `tlmgr.log` and show it only on failure.
- The local tlpdb's `00texlive.installation` record contains
  `depend opt_location:<repo>`. Use it to distinguish frozen from rolling
  repositories and to locate the remote tlpdb.

### Compilation

- Run `latexmk -pdf -f -synctex=1 -interaction=nonstopmode -file-line-error <main>`
  with `max_print_line=10000`. Otherwise, log lines wrap at 79 characters and
  can split missing-file messages.
- **Add `-g` when retrying after package installation.** Otherwise, latexmk sees
  unchanged source files and skips the build. It also incorrectly considers
  the output up to date when `.fls` refers to another TeX Live installation;
  force a rebuild in that case too. Canonicalize the TeX Live path before
  comparing it with `.fls` paths.
- With a cold cache, each fatal error may reveal only one missing file. Do not
  cap the installation loop. Never retry the same package, so the loop always
  terminates.
- Bibliography errors can make latexmk return 12 while still producing a PDF.
  Use PDF existence to decide whether a preview is available; the CLI still
  returns latexmk's exit code.
- Do not rewrite an unchanged lockfile: doing so causes endless rebuilds with
  LaTeX Workshop's `onFileChange` setting.

### Missing-file detection (log formats)

- ``File `x.sty' not found``.
- ``I can't find file `x'``; append `.tex` when there is no extension.
- `Font \x=name at 12.0pt not loadable: Metric (TFM) file not found` maps to
  `name.tfm`.

Reads of `.tfm`, `.pfb`, and `.enc` files are recorded in `.fls`, but not in the
log. Always generate the lockfile from `.fls`.

### Silent fallbacks

These cases can change the output without reporting an error. They are a key
reason for keeping a lockfile.

1. **Missing Type 1 fonts.** When METAFONT is available, it generates bitmap
   `.pk` fonts as substitutes (shown as Type 3 in the PDF), and the log contains
   `/<name>.<dpi>pk>`. Without METAFONT, the error is fatal:
   `!pdfTeX error: ... Font <name> at 600 not found`. Both cases map to
   `<name>.pfb`. Type 1 versions of EC/TC fonts are in `cm-super`, but their
   filenames differ (`tcrm1200` → `sfrm1200.pfb`). The mapping lives in `.map`
   files and cannot be resolved through tlpdb; currently, handle these fonts
   with a prefix-based special case.
2. **Multiple providers for one filename.** `*-dev` packages such as
   `latex-tools-dev` sort earlier alphabetically and can be found because the
   search path includes `tex//`. Always rank `*-dev` providers last. A symptom
   is `You have requested release ... but only release ... is available` in
   the log.
3. **`\IfFileExists` fallbacks.** A missing file selects the fallback branch
   without an error, so texup cannot detect it.

### XeLaTeX

- `scheme-infraonly` has no `xelatex`, and latexmk then fails without a log to
  scan. Install `xetex` up front, like `latex-bin` for pdfLaTeX, and lock it.
- fontspec loads fonts by name through fontconfig, which does not know TeX
  Live's font directories. Generate `<tl>/texup-fonts.conf` (system
  `/etc/fonts/fonts.conf` plus TeX Live's `fonts/{opentype,truetype,type1}`) on
  every run and pass it as `FONTCONFIG_FILE`; TeX Live's own
  `texlive-fontconfig.conf` holds absolute paths and breaks after `mv`.
  System fonts stay visible because Overleaf has fonts outside TeX Live (Noto
  CJK from Ubuntu 22.04, among others).
- `.fls` records no OpenType fonts, and xdvipdfmx's reads are not recorded at
  all. The `.xdv` holds the absolute path of every native font used (opcode 252:
  k[4] size[4] flags[2] len[1] path); lock the TeX Live ones and note the rest.
- Missing fonts: `Font ...=[file.otf]/OT at ... not loadable: Metric (TFM) file
  or installed font not found` maps to `file.otf`; `="Name"` has no tlpdb
  mapping, so match the normalized name against OpenType/TrueType file name
  prefixes (`TeX Gyre Termes` → `texgyretermes-regular.otf`).
- fontspec wraps its own info lines, so `<->"Font Name"` in the log is
  unreliable regardless of `max_print_line`.
- Dependencies often pull in fonts: `xecjk` → `ctex` → `fandol`; `fontspec` →
  `lm`.

### tlpdb

- Remote index: `<repo>/tlpkg/texlive.tlpdb.xz`. The 2023 index is approximately
  2.4 MB compressed and 18 MB uncompressed. Local index:
  `<tl>/tlpkg/texlive.tlpdb`.
- Records are separated by blank lines. Indented file entries belong to the
  preceding `runfiles`, `docfiles`, `srcfiles`, or `binfiles` section. Read only
  `runfiles` and `binfiles`; `srcfiles` may appear before them. Normalize the
  remote `RELOC/` prefix and the local `texmf-dist/` prefix to `texmf-dist/`.
- Executables are `binfiles` (`bin/x86_64-linux/<cmd>`) of a separate
  `<pkg>.x86_64-linux` record, and command names often differ from package
  names (`latexdiff-vc` → `latexdiff`). `tlmgr install <pkg>` pulls in the
  arch package.
- Do not use `tlmgr search --global --file`: it downloads the remote index on
  every invocation and is slow.
- Use the local tlpdb to map `.fls` inputs to packages, since it describes the
  actual installation. Download the remote tlpdb lazily, only when resolving
  missing files.

### Overleaf

- Overleaf uses `latexmk`, sets the job name to `output`, and compiles in
  `/compile`. These details can identify logs produced by Overleaf.
- Each Overleaf project can select its own TeX Live year. Match that year in
  `texup.toml`.

## Testing

- `cargo test`: cover each nontrivial behavior (tlpdb parsing, missing-file
  detection, `*-dev` ordering, and `.fls`-to-package mapping) with at least one
  `#[test]`, using real excerpts as fixtures.
- `cargo clippy --all-targets` should produce no warnings.
- Run end-to-end regression checks when changing installation or build flows:
  1. Keep a known-good PDF as a baseline in `tests/baseline/` (gitignored).
  2. With an empty `TEXUP_HOME`, run `texup fetch`, then `texup build`. The build
     must not trigger any missing-package installation.
  3. Render with `pdftoppm -r 150`, then compare with `compare -metric AE`.
     Require zero differing pixels and no Type 3 fonts in `pdffonts` output.

Recommended coverage: a document using a frozen TeX Live year, CJKutf8, and
BibTeX, plus a minimal document.
