//! Corrupted documents must never panic, hang, or blow up memory: decode and
//! render the fixtures after randomly damaging their decompressed archives.

use pages_cli::{doc, iwa, render};
use std::io::{Cursor, Read};

fn streams(fixture: &str) -> Vec<Vec<u8>> {
    let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
    let mut zip = zip::ZipArchive::new(Cursor::new(std::fs::read(path).unwrap())).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut raw = Vec::new();
            zip.by_index(i).unwrap().read_to_end(&mut raw).unwrap();
            iwa::decompress(&raw, usize::MAX).unwrap()
        })
        .collect()
}

/// xorshift64*, so failures reproduce from the printed seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(rng: &mut Rng, data: &mut Vec<u8>) {
    for _ in 0..1 + rng.below(16) {
        if data.is_empty() {
            return;
        }
        let at = rng.below(data.len());
        match rng.below(4) {
            0 => data[at] = rng.next() as u8,
            1 => data[at] ^= 1 << rng.below(8),
            // Long varints: absurd lengths, ids, counts and levels.
            2 => data.splice(at..at, [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]).for_each(drop),
            _ => data.truncate(at),
        }
    }
}

#[test]
fn corrupted_archives_do_not_panic() {
    let opts = render::Options { width: 60, color: true, hyperlinks: true };
    // PAGES_FUZZ_ITERS=20000 for a longer local run.
    let iterations: u64 = std::env::var("PAGES_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    for fixture in ["fixture.pages", "fixture2.pages", "fixture3.pages"] {
        let clean = streams(fixture);
        for seed in 1..=iterations {
            let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let mut damaged = clean.clone();
            let target = rng.below(damaged.len());
            mutate(&mut rng, &mut damaged[target]);
            let store = iwa::Store::from_streams(damaged.iter().map(Vec::as_slice));
            let out = std::panic::catch_unwind(|| render::render(&doc::load(&store), &opts));
            assert!(out.is_ok(), "{fixture}: panic with seed {seed}");
            assert!(out.unwrap().len() < 4 << 20, "{fixture}: runaway output with seed {seed}");
        }
    }
}
