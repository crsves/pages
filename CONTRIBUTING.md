# Contributing

Bug reports, document samples, and pull requests are all welcome.

## Reporting a document that renders wrong

Pages files often contain personal writing, so **please don't attach documents with real
content**. Instead:

1. Make a new document in Pages that reproduces the problem with placeholder text, or
2. Describe what Pages shows next to what `pages` prints (screenshots help), and say which
   Pages version saved the file (Pages ▸ About Pages).

Include the output of `pages --version`.

## Development

```sh
cargo test                       # unit, render, and corruption tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

CI runs all three on macOS and Linux, plus a build with Rust 1.85 (the minimum supported
version).

### Layout

| File | What it does |
| --- | --- |
| `src/iwa.rs` | Opens the zip or package, decompresses `.iwa` (Snappy) streams, and indexes objects |
| `src/proto.rs` | Minimal schema-less protobuf reader |
| `src/doc.rs` | Text storages and styles into blocks: paragraphs, lists, attachments |
| `src/table.rs` | Table tiles and cell records |
| `src/render.rs` | Wrapping, ANSI styling, and table drawing |

Field numbers come from the iWork protobuf schemas, which projects such as
[numbers-parser](https://github.com/masaccio/numbers-parser) and
[keynote-parser](https://github.com/psobot/keynote-parser) publish.

### Fixtures

`tests/fixtures/*.pages` are generated, never real documents. To add one:

1. Write a `.docx` with python-docx (see `tests/fixtures/mkdocx*.py`).
2. Open it in Pages and save it as `.pages`.
3. Strip everything except `Index/*.iwa`, which removes previews, images, and machine
   identifiers, the same way the existing fixtures were made.
4. Add assertions in `tests/render.rs`.

### Hostile input

The decoder must not panic, hang, or allocate without bound on corrupt files.
`tests/robustness.rs` damages the fixtures at random; run it longer before sending parser
changes:

```sh
PAGES_FUZZ_ITERS=20000 cargo test --release --test robustness
PAGES_FUZZ_ITERS=5000 cargo test --test robustness   # debug build: catches integer overflow
```
