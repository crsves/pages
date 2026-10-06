# pages

Render Apple Pages (`.pages`) documents in the terminal.

## Install

```sh
brew install crsves/tap/pages      # Homebrew (macOS, Linux)
yay -S pages                       # Arch Linux (AUR)
cargo install --git https://github.com/crsves/pages
```

Prebuilt binaries for macOS and Linux (x86_64 and arm64) are on the
[releases page](https://github.com/crsves/pages/releases).

## Usage

```sh
pages essay.pages          # print to stdout
pages -p essay.pages       # open in a pager ($PAGER, default `less -R`)
pages -w 72 essay.pages    # wrap at 72 columns
pages --color never a.pages b.pages
```

It reads the file directly. Pages.app does not have to be installed, so it also works on
Linux. It decodes Pages' IWA format (Snappy-compressed protobuf) using a small hand-written
reader, so it has no protobuf/codegen dependency.

## What renders

- Title, subtitle, and headings (from the paragraph style; also big, short, bold body
  paragraphs formatted by hand)
- Bold, italic, underline, strikethrough, and super/subscript
- Bulleted, numbered (decimal, roman, alpha, tiered), and nested lists
- Tables with box drawing; header rows are highlighted, and wide tables shrink to fit
- Hyperlinks: clickable OSC 8 links on stdout, URL shown after the link text in the pager
  or with `--color never`
- Block quotes and text boxes
- Placeholders for images, charts, and video

Colour is on when stdout is a TTY and `NO_COLOR` is unset.

## Limits

- Cell number formats (currency, percent, and so on) aren't applied. Numbers show their raw
  value, and text cells show what was typed.
- Merged table cells show as separate cells.
- Headers and footers, comments, and tracked changes are skipped.
- Pages '09 (XML) files aren't supported, and neither are tables saved by Pages versions
  before 2019 (they show a placeholder).
- Footnotes are decoded but haven't been checked against real documents yet.
- iCloud files that haven't been downloaded can't be read. Open them in Finder first.

## Build

```sh
cargo install --path .
cargo test
PAGES_FUZZ_ITERS=20000 cargo test --release --test robustness   # longer corruption run
```

Requires Rust 1.85+. Packagers can generate a man page and completions with
`pages --generate-man` and `pages --generate-completions <bash|zsh|fish|elvish|powershell>`.

`tests/fixtures/*.pages` were made by building a `.docx` with `mkdocx*.py` (python-docx),
then opening it in Pages, saving it as a `.pages` file, and keeping only the `Index/*.iwa`
archives (no previews, images, or document metadata).
