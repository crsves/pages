# Changelog

## 0.1.1 — 2026-10-06

- Rules and table borders use a mid grey, so they stay visible on dark and translucent
  terminal backgrounds
- Links whose text already is the URL (like `github.com/x`) no longer print the URL twice
- `.deb` packages for x86_64 and arm64 in each release
- `cargo binstall pages-cli` downloads the prebuilt binary
- README: `git diff`, fzf, yazi, and ranger integrations

## 0.1.0 — 2026-10-06

First release.

- Renders Pages 5+ documents (zip and package formats): headings, inline styles, lists,
  tables, links, quotes, and text boxes
- `-p` pager, `-w` width, `--color auto|always|never`, `NO_COLOR`
- Size limits and corruption tests, so hostile files fail cleanly
