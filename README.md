# texup

**A Cargo-like workflow for LaTeX.** Pin a TeX Live year per project, lock the
packages your document actually uses, and get the same PDF as Overleaf without
installing all of TeX Live.

- **`texup.toml`** pins the TeX Live year.
- **`texup.lock`** records the exact packages and revisions the build used.
- **Packages install on demand** into a cache shared by all projects.

## Installation

Requires x86_64 or aarch64 Linux with glibc 2.35 or later (Ubuntu 22.04,
Debian 12, Fedora 36, or newer), plus `perl`, `curl`, `tar`, `xz`, and, for
XeLaTeX, the fontconfig library. No existing TeX installation is needed. Desktop
installations usually have all of these; on a minimal system:

| Distribution | Command |
| --- | --- |
| Debian, Ubuntu | `sudo apt install curl perl xz-utils fontconfig` |
| Fedora | `sudo dnf install curl perl xz tar fontconfig` |
| Arch | `sudo pacman -S curl perl xz tar fontconfig` |

CI builds a pdfLaTeX, XeLaTeX, and LuaLaTeX document on each of these, on both
x86_64 and arm64 Ubuntu.

Download the binary for your machine from the
[latest release](https://github.com/chiwei085/texup/releases/latest):

```bash
mkdir -p ~/.local/bin
curl -fL -o ~/.local/bin/texup \
  "https://github.com/chiwei085/texup/releases/latest/download/texup-$(uname -m)-linux"
chmod +x ~/.local/bin/texup
texup --version
```

Make sure `~/.local/bin` is on your `PATH`. To update, rerun the same commands.
Each release also lists a `.sha256` checksum for every binary.

texup compiles through [texweft](https://github.com/chiwei085/texweft), a build
driver that reports results as structured messages. For now texweft must be
installed separately and be on your `PATH` (texup stops with
`texup: texweft not found on PATH` otherwise); texup will download it itself once
texweft publishes release binaries:

```bash
cargo install --git https://github.com/chiwei085/texweft
```

To build from source instead, install Rust 1.88 or later and run:

```bash
cargo install --git https://github.com/chiwei085/texup
```

This installs `texup` to `~/.cargo/bin`; to update, rerun the command with
`--force`.

## Quick start

```bash
cd my-paper
texup init --year 2025
texup build
```

Use the same year as your Overleaf project (Menu → TeX Live version). The first
build installs a minimal TeX Live, adds the packages the document needs, and
writes `texup.lock`. Commit both `texup.toml` and `texup.lock`.

`--year` can be omitted if the project has a `.log` whose first line names the
version, such as `(TeX Live 2023)`.

## Commands

Every command takes an optional project directory, defaulting to the current one.

| Command | Description |
| --- | --- |
| `texup init [dir] [--year Y]` | Create `texup.toml`. |
| `texup build [dir] [--locked]` | Compile the document and update `texup.lock`. With `--locked`, fail instead of installing a package or changing `texup.lock`. |
| `texup watch [dir]` | Build, then rebuild whenever a file the document reads changes, installing missing packages as they come up. It does not open a PDF viewer; point your viewer at the PDF, which it reloads. |
| `texup fetch [dir]` | Install the packages in `texup.lock` without compiling, e.g. before going offline. |
| `texup clean [dir]` | Remove the build outputs texweft made (a `.bbl` you ship without its `.bib` is kept). |
| `texup why <package> [dir]` | List the files from `<package>` that the last build read, i.e. why it is in `texup.lock`. |
| `texup exec <cmd> [args…]` | Run a TeX Live command from the project's year, installing its package first if needed. Runs in the current directory. |
| `texup --version` | Print the texup version. |

`texup exec` gives access to any TeX Live tool without a system-wide install,
for example:

```bash
texup exec latexdiff-vc --git -r v1 --pdf main.tex   # marked-up diff against tag v1
texup exec texcount -inc main.tex                    # word count
texup exec chktex main.tex                           # lint
```

Packages installed this way are not added to `texup.lock`, which only records
what the document itself reads.

## How it works

`texup build` runs these steps:

1. Install the year's TeX Live infrastructure if it is not cached yet.
2. Install any packages from `texup.lock` that are missing.
3. Run `texweft build`. When it reports a missing file, font, or program, install
   the package that provides it and rerun, until nothing is missing. A Type 1
   font that TeX silently replaced with a bitmap is installed the same way.
4. Write `texup.lock` from texweft's list of inputs: the packages owning every
   TeX Live file the build read (fonts included), the packages providing the
   programs it ran (such as `biber`), plus `latex-bin` and `bibtex`.

The lockfile reflects what the document uses, not what happens to be in the
shared cache. It is rewritten only when its contents change, so file watchers
are not triggered needlessly.

How exactly a lockfile can be restored depends on the year:

| Year | Repository | `texup fetch` |
| --- | --- | --- |
| Past years, e.g. 2023 | Frozen snapshot (`tlnet-final`) | Restores the exact revisions. |
| Current year | Rolling (`tlnet`) | Installs the latest revisions and warns about any that differ from the lockfile. |

## Configuration

### `texup.toml`

```toml
[texlive]
year = 2023

[build]
main = "paper.tex"
engine = "pdflatex"
```

| Field | Required | Default |
| --- | --- | --- |
| `texlive.year` | Yes | — |
| `build.main` | No | `main.tex` if it exists, otherwise the only top-level `.tex` file containing `\documentclass` |
| `build.engine` | No | `pdflatex`; also `xelatex` or `lualatex` (see [XeLaTeX and LuaLaTeX](#xelatex-and-lualatex)) |

### `texup.lock`

Generated by `texup build`. Do not edit it by hand.

```toml
version = 1
texlive = "2023/tlnet-final"

[[package]]
name = "tools"
revision = 68941
```

## Overleaf compatibility

- texweft runs the engines with the options Overleaf's `latexmk -pdf -f
  -synctex=1 -interaction=nonstopmode -file-line-error` uses, and keeps the
  outputs next to the sources.
- Like Overleaf, a document with errors still gets a complete PDF: BibTeX and
  reruns still happen, so citations and cross-references resolve. texup then
  exits with a nonzero code and prints an error summary.
- Overleaf ignores `texup.toml` and `texup.lock`.
- texup never creates a `latexmkrc`, which Overleaf would pick up.
- Besides the usual auxiliary files, a build leaves `<main>.texweft.json`
  (texweft's record of the last successful build) and `.<main>.texweft.lock`
  next to the sources. Like `.aux` and `.log`, keep them out of git and out of
  uploads to Overleaf:

  ```gitignore
  *.texweft.json
  .*.texweft.lock
  ```

## VS Code (LaTeX Workshop)

Let texup compile and LaTeX Workshop preview. Add this to your VS Code settings:

```jsonc
{
  "latex-workshop.latex.tools": [
    {
      "name": "texup",
      "command": "bash",
      "args": ["-c", "texup build \"$0\"; [ -f \"$1\" ]", "%DIR%", "%DIR%/%DOCFILE%.pdf"]
    }
  ],
  "latex-workshop.latex.recipes": [{ "name": "texup", "tools": ["texup"] }],
  "latex-workshop.latex.autoBuild.run": "onFileChange"
}
```

The recipe succeeds whenever a PDF exists, so bibliography errors do not block
the preview. If VS Code was not started from a shell, `~/.local/bin` or
`~/.cargo/bin` may be missing from its `PATH`; use the absolute path to `texup`
instead.

## CI

`texup build --locked` fails if the committed `texup.lock` is missing or does
not match what the build uses, for example after adding a `\usepackage`
without committing the updated lockfile. A GitHub Actions workflow:

```yaml
name: pdf
on: [push, pull_request]
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: actions/cache@v6
        with:
          path: ~/.cache/texup
          key: texup-${{ hashFiles('texup.toml', 'texup.lock') }}
      - run: |
          curl -fsSL -o texup https://github.com/chiwei085/texup/releases/latest/download/texup-x86_64-linux
          chmod +x texup
      - run: ./texup build --locked
      - uses: actions/upload-artifact@v7
        with:
          name: pdf
          path: "*.pdf"
```

The cache key changes with the lockfile, so each cache holds exactly the locked
packages. For the current (rolling) year, an uncached run installs the latest
revisions, and `--locked` fails if any differ from the lockfile.

## XeLaTeX and LuaLaTeX

Set `engine = "xelatex"` or `engine = "lualatex"` to match the compiler of the
Overleaf project. fontspec can find fonts by name; texup makes TeX Live's fonts
and your system fonts available to it.

- TeX Live fonts are installed on demand like packages, whether loaded by file
  name (`\setmainfont{texgyretermes-regular.otf}`) or by name
  (`\setmainfont{TeX Gyre Termes}`). Names are matched against font file names,
  so a family whose files are named differently may not be found; use its file
  name instead.
- Overleaf also has fonts that are not part of TeX Live, notably Noto CJK
  (`Noto Serif CJK TC` and others) and some Microsoft core fonts. Install them on
  your system, e.g. `apt install fonts-noto-cjk`. texup prints a note for every
  non-TeX Live font the document uses, since these are not in `texup.lock` and
  Overleaf must provide them too.

## Cache

The cache lives in `~/.cache/texup`, or in `$TEXUP_HOME` if set:

```text
~/.cache/texup/
├── tl/<year>/
├── tlpdb/<year>.tlpdb
└── tlmgr.log
```

`tl/<year>/` holds one TeX Live installation per year and can be moved with `mv`.
`tlpdb/` caches the package index used to resolve missing files. `tlmgr.log`
keeps the full `tlmgr` output for troubleshooting.

## Limitations

- Only x86_64 Linux, pdfLaTeX, XeLaTeX, and LuaLaTeX are supported. Overleaf's
  "LaTeX" compiler (DVI output) is not.
- A file probed with `\IfFileExists` silently falls back when missing, so texup
  cannot detect it.
- Missing Type 1 fonts are resolved by looking up `<font>.pfb`. EC/TC fonts such
  as `tcrm1200` are special-cased to `cm-super`; other fonts whose Type 1 file has
  a different name are not resolved automatically.
- Documents that need makeindex, glossaries, or shell escape stop with an
  "unsupported" error instead of producing a possibly incomplete PDF; texweft
  does not run those tools yet.

## License

[MIT](LICENSE)
