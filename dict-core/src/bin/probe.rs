//! Diagnostic binary: open a real dictionary, print its fingerprint, and time
//! a batch of lookups. Run with:
//!
//! ```text
//! cargo run --release --offline --bin probe -- "D:\path\to\dict.ifo" [word...]
//! ```

use std::time::Instant;

use dict_core::{fingerprint, Dictionary};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(ifo) = args.next() else {
        eprintln!("usage: probe <dict.ifo> [word ...]");
        std::process::exit(2);
    };
    let words: Vec<String> = args.collect();
    let words = if words.is_empty() {
        [
            "hello",
            "dictionary",
            "apple",
            "translation",
            "computer-based message system",
            "APPLE",
            "the apple",
            "apples,",
            "notawordxyz",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    } else {
        words
    };

    let t0 = Instant::now();
    let dict = match Dictionary::open(&ifo) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("open failed: {e}");
            std::process::exit(1);
        }
    };
    let open_ms = t0.elapsed().as_secs_f64() * 1000.0;

    println!("== dictionary ==");
    println!("path            {}", dict.path.display());
    println!("bookname        {}", dict.ifo.bookname);
    println!("sametypeseq     {:?}", dict.ifo.sametypesequence);
    println!("entries         {}", dict.entry_count());
    println!("index bytes     {}", dict.index_bytes());
    println!("offset table    {:.1} MiB", dict.offsets_bytes() as f64 / 1048576.0);
    println!("open + scan     {open_ms:.1} ms (scan {} us)", dict.scan_micros());

    let fp = fingerprint(&dict);
    println!("\n== direction ==");
    println!(
        "{:?} ({}) confidence {:.2}",
        fp.direction,
        fp.direction.label(),
        fp.confidence
    );
    println!(
        "headwords han {:.3} ({} samples), definitions han {:.3} ({} samples)",
        fp.headword_han_ratio, fp.headword_samples, fp.definition_han_ratio, fp.definition_samples
    );

    println!("\n== lookups ==");
    let mut total_us = 0.0f64;
    let mut hits = 0usize;
    for word in &words {
        let t = Instant::now();
        let hit = dict.lookup(word);
        let us = t.elapsed().as_secs_f64() * 1_000_000.0;
        total_us += us;
        if hit.is_some() {
            hits += 1;
        }
        match hit {
            Some(l) => {
                let def = l.entry.definition.replace('\n', " / ");
                let short: String = def.chars().take(70).collect();
                println!(
                    "  {word:<34} HIT  {:>6.1} us  [{}] -> {} : {}",
                    us, l.reason, l.entry.headword, short
                );
            }
            None => println!("  {word:<34} miss {:>6.1} us", us),
        }
    }
    println!(
        "\n{hits}/{} hits, avg {:.1} us/lookup",
        words.len(),
        total_us / words.len() as f64
    );
}
