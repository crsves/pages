use std::process::Command;

fn render(fixture: &str, extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_pages"))
        .args(["-w", "70"])
        .args(extra)
        .arg(format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("run pages");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn headings_lists_and_tables() {
    let out = render("fixture.pages", &[]);
    for expected in [
        "  The Fixture Essay\n  ━━━━━━━━━━━━━━━━━\n  A subtitle line\n",
        "  • First bullet\n  • Second bullet\n     • Nested bullet\n  • Third bullet\n",
        "  1. Step one\n  2. Step two\n  3. Step three\n",
        "  │ Name  │ Role    │ Year │\n  │ Ada   │ Analyst │ 1843 │\n",
        "  │ A block quote of some wisdom.\n",
        "emoji 🎉 to test widths",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
    assert!(!out.contains('\u{FFFC}'));
    assert!(out.lines().all(|l| unicode_width(l) <= 70), "line too wide:\n{out}");
}

#[test]
fn nested_numbering_and_scripts() {
    let out = render("fixture2.pages", &[]);
    assert!(out.contains("  1. lvl0 a\n     1. lvl1 a\n        1. lvl2 a\n     2. lvl1 b\n  2. lvl0 b\n"), "{out}");
    assert!(out.contains("H₂O"));
    // UTF-16 offsets: style runs after astral-plane emoji stay aligned.
    assert!(out.contains("Emoji 🎉🎉 first then BOLDWORD after."));
}

#[test]
fn inline_styles_in_color() {
    let out = render("fixture.pages", &["--color", "always"]);
    assert!(out.contains("\x1b[1mbold\x1b[0m"));
    assert!(out.contains("\x1b[3mitalic\x1b[0m"));
    assert!(out.contains("\x1b[4munderlined\x1b[0m"));
    assert!(out.contains("\x1b[1;3mbold italic\x1b[0m"));
    let out = render("fixture2.pages", &["--color", "always"]);
    assert!(out.contains("\x1b[1mBOLDWORD\x1b[0m"));
    assert!(out.contains("\x1b[9mgone\x1b[0m"));
}

#[test]
fn links_and_header_rows() {
    let plain = render("fixture3.pages", &[]);
    assert!(plain.contains("See the docs (https://example.com/docs) for more."), "{plain}");
    assert!(plain.contains("  │ Item   │ Qty │\n  ├────────┼─────┤\n"), "{plain}");
    let color = render("fixture3.pages", &["--color", "always"]);
    assert!(color.contains("\x1b]8;;https://example.com/docs\x1b\\"));
}

#[test]
fn rejects_non_pages_files() {
    let out = Command::new(env!("CARGO_BIN_EXE_pages"))
        .arg(format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR")))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a Pages document"));
}

fn unicode_width(s: &str) -> usize {
    // Good enough for the fixtures: emoji and CJK count double.
    s.chars().map(|c| if (c as u32) >= 0x1F000 || ('\u{3000}'..='\u{9FFF}').contains(&c) { 2 } else { 1 }).sum()
}
