# texup — Development guide

See [README.md](README.md) for user documentation. This file records design
principles and verified TeX Live behavior. Read it before changing related logic.

## Code structure

- `src/main.rs`: CLI, manifest and lockfile handling, TeX Live installation
  (`install-tl` / `tlmgr`), and the build loop that runs texweft and installs
  what it reports missing.
- `src/tlpdb.rs`: `texlive.tlpdb` parsing; filename-to-provider lookup with
  `*-dev` packages ranked last; full-path-to-package lookup; command-to-package
  lookup for `texup exec`.
- `src/scan.rs`: TeX Live files read, from `.fls`, for `texup why`.
- `tests/fixtures/`: excerpts from real tlpdb and `.fls` files.

## Principles

- Comments explain intent, constraints, and non-obvious behavior; do not narrate the code.
- Scope: the manifest/lockfile contract, tlpdb parsing, invoking `install-tl`,
  `tlmgr`, and texweft, installing missing packages, and generating the lockfile
  from texweft's `dep-info`. Log parsing, rerun decisions, and engine quirks
  belong to texweft (`~/chiwei/texweft`); texup reads only its JSON Lines
  messages and never depends on it as a crate.
- Out of scope: modifying TeX engines, FUSE or lazy filesystems, and reimplementing
  TeX Live installation semantics. Leave `.fmt` generation and `updmap` to
  `tlmgr`. These alternatives were evaluated and were not worth the complexity.
- Support only x86_64 and aarch64 Linux with glibc (TeX Live's `x86_64-linux` and
  `aarch64-linux`), and pdfLaTeX, XeLaTeX, and LuaLaTeX. The platform is fixed at
  compile time; `cargo install` builds on the host.
- Keep dependencies limited to `serde`, `toml`, and `serde_json` (texweft's
  messages). Use `curl`, `tar`, and `xz` directly for downloads and extraction.
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

- Run `texweft build <main> --engine <e> --outdir . --message-format=json`
  through `tex()`, which puts the TeX Live `bin` first on `PATH` and sets
  `max_print_line` and `FONTCONFIG_FILE` (XeLaTeX font lookup needs it).
  `--outdir .` keeps outputs next to the sources, where LaTeX Workshop's SyncTeX
  and viewer settings, `texup why`, and existing projects expect them. texweft
  must be on `PATH`; report a missing one as `texup: texweft not found on PATH`.
- Map `missing.kind` to packages: `file` and `font-file` by file name,
  `font-name` by font name, `font-outline` by `<name>.pfb` (with the cm-super
  special case), `program` by command name. Install fatal entries and
  substituted outline fonts (`font-outline` with `substitute: "pk"`); ignore a
  `.bib` replaced by a shipped `.bbl`. `library` is a system library: print its
  raw loader line and stop.
- No force flag is needed after installing: texweft keeps no state for a
  `missing` build, so the retry runs. It does reuse a `converged` build, and a
  `failed` one the document caused, while every input it read is unchanged;
  installing an outline font still triggers a rebuild, because updmap rewrites
  `pdftex.map`, which the build read. `texweft watch` also rebuilds when `ls-R`
  changes.
- With a cold cache, each fatal error may reveal only one missing file. Do not
  cap the installation loop. Never retry the same package, so the loop always
  terminates.
- Preview availability is separate from the outcome: print `preview` whenever
  texweft reports one (a document with errors still gets citations and
  references resolved, as on Overleaf), and still exit nonzero unless the
  outcome is `converged`.
- Lock the owners of `dep-info.inputs` and the packages providing
  `dep-info.programs`: inputs name BibTeX's or Biber's files, not the binaries,
  and an on-demand `biber` must be locked for `--locked` on a fresh cache. An
  up-to-date build (0 steps) reports both lists too, so a deleted or outdated
  lockfile is restored, and `--locked` checked, without rebuilding.
- With `--locked`, any missing entry mapping to a package not in `texup.lock`
  dies, whatever its kind.
- Do not rewrite an unchanged lockfile: doing so causes endless rebuilds with
  LaTeX Workshop's `onFileChange` setting.
- `texup watch` reads `texweft watch`'s stream and installs what a build reports
  missing; texweft rebuilds by itself once tlmgr rewrites `ls-R`. tlmgr writes
  `ls-R` before updmap and fmtutil, so installs hold texweft's output lock
  (`.<job>.texweft.lock`, flock) and texweft waits. It opens no viewer
  (latexmk -pvc did).

### Silent fallbacks

These cases can change the output without reporting an error. They are a key
reason for keeping a lockfile.

1. **Missing Type 1 fonts.** When METAFONT is available, it generates bitmap
   `.pk` fonts as substitutes (shown as Type 3 in the PDF), and the log contains
   `/<name>.<dpi>pk>`. Without METAFONT, the error is fatal:
   `!pdfTeX error: ... Font <name> at 600 not found`. texweft reports both as
   `font-outline` with the TeX font name; texup maps it to `<name>.pfb`. Type 1 versions of EC/TC fonts are in `cm-super`, but their
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

- `scheme-infraonly` has no `xelatex`. Install `xetex` up front, like
  `latex-bin` for pdfLaTeX, and lock it.
- fontspec loads fonts by name through fontconfig, which does not know TeX
  Live's font directories. Generate `<tl>/texup-fonts.conf` (system
  `/etc/fonts/fonts.conf` plus TeX Live's `fonts/{opentype,truetype,type1}`) on
  every run and pass it as `FONTCONFIG_FILE`; TeX Live's own
  `texlive-fontconfig.conf` holds absolute paths and breaks after `mv`.
  System fonts stay visible because Overleaf has fonts outside TeX Live (Noto
  CJK from Ubuntu 22.04, among others).
- `.fls` records no OpenType fonts, and xdvipdfmx's reads are not recorded at
  all. texweft reads them from the `.xdv` (`dep-info` entries with `via: "xdv"`);
  lock the TeX Live ones and note the rest.
- Missing fonts: `Font ...=[file.otf]/OT at ... not loadable: Metric (TFM) file
  or installed font not found` maps to `file.otf`; `="Name"` has no tlpdb
  mapping, so match the normalized name against the stem of OpenType/TrueType
  file names before the first `-`, `.`, or `_` (`TeX Gyre Termes` →
  `texgyretermes-regular.otf`). Require equality: `context` ships
  `TeXGyreTermesMathCompanion-Regular.otf` and sorts before `tex-gyre`.
- fontspec wraps its own info lines, so `<->"Font Name"` in the log is
  unreliable regardless of `max_print_line`.
- Dependencies often pull in fonts: `xecjk` → `ctex` → `fandol`; `fontspec` →
  `lm`.

### LuaLaTeX

- `latex-bin` already builds the `lualatex` format; no extra engine package.
- luaotfload keeps its own font database under `TEXMFVAR`, scans system fonts
  itself, and rescans when a requested font is missing, so fonts installed by
  `tlmgr` are found on the next run.
- A cold luaotfload database scan records unrelated outline fonts in `.fls`.
  texweft must exclude those entries and report embedded fonts from the
  `</abs/path.otf>` log lines, keeping cold and warm lockfiles identical.
- Missing fonts: `...=[file.otf]:mode=node;... at 10pt not loadable: metric data
  not found or bad` (file) and `...=name:TeX Gyre Termes:mode=...` (name).

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

- `cargo test`: cover each nontrivial behavior (tlpdb parsing, `*-dev`
  ordering, and `.fls`-to-package mapping) with at least one `#[test]`, using
  real excerpts as fixtures. Log parsing is texweft's and tested there.
- `cargo clippy --all-targets` should produce no warnings.
- Run end-to-end regression checks when changing installation or build flows:
  1. Keep a known-good PDF as a baseline in `tests/baseline/` (gitignored).
  2. With an empty `TEXUP_HOME`, run `texup fetch`, then `texup build`. The build
     must not trigger any missing-package installation.
  3. Render with `pdftoppm -r 150`, then compare with `compare -metric AE`.
     Require zero differing pixels and no Type 3 fonts in `pdffonts` output.

Recommended coverage: a document using a frozen TeX Live year, CJKutf8, and
BibTeX, plus a minimal document.

CI (`.github/workflows/ci.yml`) runs clippy and the tests, then builds
`tests/e2e/<engine>/` with `--locked` on Ubuntu x86_64 and arm64 and in Debian,
Fedora, and Arch containers, with texweft's release binary (pinned by version and
sha256). After changing those documents, regenerate their
`texup.lock` with an empty `TEXUP_HOME`.

Releases: bump `version` in `Cargo.toml`, then push a `v<version>` tag.
`.github/workflows/release.yml` builds `texup-<platform>` binaries on
`ubuntu-22.04` and `ubuntu-22.04-arm` (oldest runners, so the binaries need only
glibc 2.35, as the README states) and attaches them to a draft release; write
the notes and publish it. Run the workflow manually to check the binaries
without releasing.

### Linux dependencies

- texup needs `perl`, `curl`, `tar`, and `xz`; report a missing one by name.
- TeX Live's `xetex` links the system's `libfontconfig` (and freetype, png,
  etc.); `pdftex` and `luahbtex` need only libc. Without fontconfig,
  `tlmgr install xetex` already fails in fmtutil with only
  `ERROR: xetex.fmt/xetex` at the end; the cause (`error while loading shared
  libraries`) is earlier in the output. If the library disappears later, xelatex
  fails before writing a log; texweft then reports `missing` with
  `kind: "library"` (and never the previous run's log). Search this run's part
  of `tlmgr.log` for the loader line, report texweft's raw line for builds.
  Installing the library and rerunning recovers.
