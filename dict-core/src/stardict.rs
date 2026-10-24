//! StarDict reader: parses `.ifo`, maps `.idx`/`.dict`, builds a u32 offset
//! table, and answers lookups with a bounded fallback chain.
//!
//! Record layout of `.idx` (version 2.4.2, all integers big-endian):
//!
//! ```text
//! headword bytes | 0x00 | u32 offset into .dict | u32 byte length
//! ```
//!
//! The 8 numeric bytes frequently contain 0x00, so records can only be found
//! through the offset table built by the one-time scan below - never by
//! searching for NUL bytes.

use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

use crate::mmap::MappedFile;
use crate::normalize::candidates;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfoInfo {
    pub bookname: String,
    pub wordcount: u32,
    pub idxfilesize: u64,
    pub sametypesequence: String,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub headword: String,
    pub definition: String,
}

#[derive(Debug, Clone)]
pub struct Lookup {
    pub entry: Entry,
    /// The term that actually matched (may differ from the query).
    pub matched: String,
    /// Which fallback step produced the match.
    pub reason: &'static str,
    /// How many candidates were tried before the hit.
    pub attempts: usize,
}

/// One anchor every N records. Storing an offset per record costs 4 bytes x
/// 3.4M = 13.6 MB of private memory; an anchor every 32 records costs 0.43 MB
/// and at most 32 sequential record comparisons per lookup (a few microseconds).
const ANCHOR_STRIDE: usize = 32;

pub struct Dictionary {
    pub path: PathBuf,
    pub ifo: IfoInfo,
    idx: MappedFile,
    dict: MappedFile,
    /// File offset of every `ANCHOR_STRIDE`-th record.
    anchors: Vec<u32>,
    /// Total record count (not the anchor count).
    count: usize,
    scan_micros: u128,
}

impl Dictionary {
    /// Open a StarDict dictionary from its `.ifo` path.
    pub fn open(ifo_path: impl AsRef<Path>) -> io::Result<Self> {
        let ifo_path = ifo_path.as_ref();

        // File names may contain dots ("stardict-ecdict-2.4.2"), so strip the
        // suffix as text instead of using Path::with_extension, which would
        // treat "2" as the extension.
        let name = ifo_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad .ifo name"))?;
        let base_name = name
            .strip_suffix(".ifo")
            .or_else(|| name.strip_suffix(".IFO"))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "path must end in .ifo")
            })?;
        let stem = ifo_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
            .join(base_name);

        let ifo_text = std::fs::read_to_string(ifo_path)?;
        let ifo = parse_ifo(&ifo_text);

        let idx_path = stem.with_file_name(format!("{base_name}.idx"));
        let dict_path = stem.with_file_name(format!("{base_name}.dict"));
        let dz_path = stem.with_file_name(format!("{base_name}.dict.dz"));
        if !dict_path.exists() && dz_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "compressed .dict.dz is not supported",
            ));
        }

        let idx = MappedFile::open(&idx_path)?;
        let dict = MappedFile::open(&dict_path)?;

        let started = std::time::Instant::now();
        // Stream the index instead of touching it through the mapping: mapping
        // 82 MB of records makes those pages resident and inflates the working
        // set by that much, which we cannot afford (100 MB budget).
        let (anchors, count) = scan_offsets(&idx_path, 1 << 20)?;
        let scan_micros = started.elapsed().as_micros();

        Ok(Self {
            path: stem,
            ifo,
            idx,
            dict,
            anchors,
            count,
            scan_micros,
        })
    }

    pub fn entry_count(&self) -> usize {
        self.count
    }

    pub fn scan_micros(&self) -> u128 {
        self.scan_micros
    }

    pub fn index_bytes(&self) -> usize {
        self.idx.len()
    }

    /// Anchor table size in bytes.
    pub fn offsets_bytes(&self) -> usize {
        self.anchors.len() * std::mem::size_of::<u32>()
    }

    /// Parse the record starting at `start`; returns the next record position.
    fn record_at_offset(&self, start: usize) -> Option<(&[u8], u32, u32, usize)> {
        let idx = self.idx.as_slice();
        let tail = idx.get(start..)?;
        let end = start + tail.iter().position(|&b| b == 0)?;
        let word = &idx[start..end];
        let off = u32::from_be_bytes(idx.get(end + 1..end + 5)?.try_into().ok()?);
        let size = u32::from_be_bytes(idx.get(end + 5..end + 9)?.try_into().ok()?);
        Some((word, off, size, end + 9))
    }

    fn record_at(&self, i: usize) -> Option<(&[u8], u32, u32)> {
        let block = i / ANCHOR_STRIDE;
        let skip = i % ANCHOR_STRIDE;
        let mut pos = *self.anchors.get(block)? as usize;
        for _ in 0..skip {
            pos = self.record_at_offset(pos)?.3;
        }
        let (word, off, size, _) = self.record_at_offset(pos)?;
        Some((word, off, size))
    }

    /// Headword bytes at position `i` (for sampling / diagnostics).
    pub fn headword_at(&self, i: usize) -> Option<&[u8]> {
        self.record_at(i).map(|(w, _, _)| w)
    }

    /// Byte range of the definition for entry `i`.
    pub fn definition_range_at(&self, i: usize) -> Option<(u32, u32)> {
        self.record_at(i).map(|(_, o, s)| (o, s))
    }

    /// Decode the definition stored at `offset..offset+size` in `.dict`.
    pub fn definition_at(&self, offset: u32, size: u32) -> Option<String> {
        let start = offset as usize;
        let end = start.saturating_add(size as usize);
        self.dict
            .as_slice()
            .get(start..end)
            .map(|bytes| decode_entry(bytes, &self.ifo.sametypesequence))
    }

    fn read_definition(&self, offset: u32, size: u32) -> String {
        self.definition_at(offset, size).unwrap_or_default()
    }

    /// Exact, ASCII-case-insensitive lookup. No fallbacks.
    pub fn lookup_exact(&self, term: &str) -> Option<Entry> {
        // Fold the query the same way dictionary headwords are folded.
        let query: Vec<u8> = term.bytes().map(|b| b.to_ascii_lowercase()).collect();
        if self.anchors.is_empty() {
            return None;
        }

        // Last anchor whose record sorts <= the query.
        let (mut lo, mut hi) = (0usize, self.anchors.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (word, _, _, _) = self.record_at_offset(self.anchors[mid] as usize)?;
            if cmp_ascii_lower(word, &query) == Ordering::Greater {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let block = lo.saturating_sub(1);

        // Scan that block (at most ANCHOR_STRIDE records).
        let mut pos = self.anchors[block] as usize;
        for _ in 0..ANCHOR_STRIDE {
            let (word, off, size, next) = self.record_at_offset(pos)?;
            match cmp_ascii_lower(word, &query) {
                Ordering::Equal => {
                    return Some(Entry {
                        headword: String::from_utf8_lossy(word).into_owned(),
                        definition: self.read_definition(off, size),
                    })
                }
                Ordering::Greater => return None,
                Ordering::Less => pos = next,
            }
        }
        None
    }

    /// Lookup with the bounded fallback chain. Returns the first hit together
    /// with the step that produced it.
    pub fn lookup(&self, query: &str) -> Option<Lookup> {
        for (attempt, cand) in candidates(query).into_iter().enumerate() {
            if let Some(entry) = self.lookup_exact(&cand.term) {
                return Some(Lookup {
                    entry,
                    matched: cand.term,
                    reason: cand.reason,
                    attempts: attempt + 1,
                });
            }
        }
        None
    }
}

/// Scan `.idx` and record the file offset of every `ANCHOR_STRIDE`-th record,
/// plus the total record count.
fn scan_offsets(path: &Path, chunk_size: usize) -> io::Result<(Vec<u32>, usize)> {
    use std::io::Read;

    let file = std::fs::File::open(path)?;
    // Roughly one record per 24 bytes.
    let expected = file.metadata().map(|m| m.len() as usize / 24).unwrap_or(0);
    let mut reader = std::io::BufReader::with_capacity(chunk_size, file);
    let mut anchors: Vec<u32> = Vec::with_capacity(expected / ANCHOR_STRIDE + 16);
    let mut count: usize = 0;

    let mut buf: Vec<u8> = Vec::with_capacity(chunk_size * 2);
    let mut base: u64 = 0; // file offset of buf[0]
    let mut cur: usize = 0; // parse cursor inside buf
    let mut chunk = vec![0u8; chunk_size];

    loop {
        let read = reader.read(&mut chunk)?;
        let eof = read == 0;
        if !eof {
            buf.extend_from_slice(&chunk[..read]);
        }

        // Parse every complete record currently buffered.
        loop {
            let Some(rel) = buf[cur..].iter().position(|&b| b == 0) else {
                break;
            };
            let nul = cur + rel;
            // A record is headword | NUL | u32 | u32.
            if buf.len() < nul + 9 {
                break;
            }
            if count % ANCHOR_STRIDE == 0 {
                anchors.push((base + cur as u64) as u32);
            }
            count += 1;
            cur = nul + 9;
        }

        if cur > 0 {
            buf.drain(0..cur);
            base += cur as u64;
            cur = 0;
        }

        if eof {
            break;
        }
    }

    Ok((anchors, count))
}

fn parse_ifo(text: &str) -> IfoInfo {
    let mut bookname = String::new();
    let mut wordcount = 0u32;
    let mut idxfilesize = 0u64;
    let mut sametypesequence = String::new();

    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("bookname=") {
            bookname = v.to_string();
        } else if let Some(v) = line.strip_prefix("wordcount=") {
            wordcount = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("idxfilesize=") {
            idxfilesize = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("sametypesequence=") {
            sametypesequence = v.trim().to_string();
        }
    }

    IfoInfo {
        bookname,
        wordcount,
        idxfilesize,
        sametypesequence,
    }
}

/// Decode one `.dict` entry. For `sametypesequence=m` the bytes are plain
/// UTF-8 text. Otherwise blocks are `type byte | payload`, NUL separated.
fn decode_entry(bytes: &[u8], sametypesequence: &str) -> String {
    let mut out = String::new();

    if sametypesequence == "m" {
        out.push_str(&String::from_utf8_lossy(bytes));
    } else {
        // Only plain-text types are rendered; anything else is skipped.
        let mut rest = bytes;
        while !rest.is_empty() {
            let ty = rest[0];
            let payload = &rest[1..];
            let (chunk, tail) = match payload.iter().position(|&b| b == 0) {
                Some(p) => (&payload[..p], &payload[p + 1..]),
                None => (payload, &[][..]),
            };
            if ty == b'm' || ty == b'l' || ty == b'g' || ty == b'x' {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&String::from_utf8_lossy(chunk));
            }
            rest = tail;
        }
    }

    out.trim_matches('\u{feff}').trim().to_string()
}

/// Compare a dictionary headword against an already-lowercased query, using
/// ASCII case folding so that "Apple" and "apple" match.
fn cmp_ascii_lower(word: &[u8], lower_query: &[u8]) -> Ordering {
    let n = word.len().min(lower_query.len());
    for i in 0..n {
        let a = word[i].to_ascii_lowercase();
        let b = lower_query[i];
        if a != b {
            return a.cmp(&b);
        }
    }
    word.len().cmp(&lower_query.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDict {
        dir: PathBuf,
    }

    impl Drop for TempDict {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Build a tiny StarDict dictionary on disk, sorted by lowercase order.
    fn build_temp(entries: &[(&str, &str)]) -> TempDict {
        let mut sorted: Vec<_> = entries.to_vec();
        sorted.sort_by_key(|(w, _)| w.to_ascii_lowercase());

        let mut idx = Vec::new();
        let mut dict = Vec::new();
        for (word, definition) in &sorted {
            let offset = dict.len() as u32;
            dict.extend_from_slice(definition.as_bytes());
            idx.extend_from_slice(word.as_bytes());
            idx.push(0);
            idx.extend_from_slice(&offset.to_be_bytes());
            idx.extend_from_slice(&(definition.len() as u32).to_be_bytes());
        }

        // NOT std::env::temp_dir(): on this machine the process token cannot
        // create directories under %TEMP% (another sandbox owns its ACL).
        // The crate's own target/ directory is always writable.
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        let dir = base.join(format!(
            "dict-core-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let stem = dir.join("testdict");
        std::fs::write(
            stem.with_extension("ifo"),
            format!(
                "StarDict's dict ifo file\nversion=2.4.2\nwordcount={}\nidxfilesize={}\nbookname=test\nsametypesequence=m\n",
                sorted.len(),
                idx.len()
            ),
        )
        .unwrap();
        std::fs::write(stem.with_extension("idx"), &idx).unwrap();
        std::fs::write(stem.with_extension("dict"), &dict).unwrap();

        TempDict { dir }
    }

    fn sample() -> TempDict {
        build_temp(&[
            ("apple", "n. 苹果"),
            ("apple juice", "苹果汁"),
            ("run", "v. 跑"),
            ("the", "art. 定冠词"),
            ("Zeal", "n. 热情"),
        ])
    }

    #[test]
    fn opens_and_counts_entries() {
        let t = sample();
        let d = Dictionary::open(t.dir.join("testdict.ifo")).unwrap();
        assert_eq!(d.entry_count(), 5);
        assert_eq!(d.ifo.bookname, "test");
        assert_eq!(d.ifo.sametypesequence, "m");
    }

    #[test]
    fn exact_lookup_is_case_insensitive() {
        let t = sample();
        let d = Dictionary::open(t.dir.join("testdict.ifo")).unwrap();
        assert_eq!(d.lookup_exact("apple").unwrap().definition, "n. 苹果");
        assert_eq!(d.lookup_exact("APPLE").unwrap().definition, "n. 苹果");
        assert_eq!(d.lookup_exact("Zeal").unwrap().headword, "Zeal");
        assert!(d.lookup_exact("missing").is_none());
    }

    #[test]
    fn handles_phrases_with_spaces() {
        let t = sample();
        let d = Dictionary::open(t.dir.join("testdict.ifo")).unwrap();
        assert_eq!(d.lookup("apple juice").unwrap().entry.definition, "苹果汁");
    }

    #[test]
    fn fallback_chain_reports_reason() {
        let t = sample();
        let d = Dictionary::open(t.dir.join("testdict.ifo")).unwrap();

        let hit = d.lookup("the apple,").unwrap();
        assert_eq!(hit.entry.definition, "n. 苹果");
        assert!(hit.attempts >= 2, "should not match on the first candidate");
        assert!(
            hit.reason == "去冠词" || hit.reason == "去首尾标点" || hit.reason == "忽略大小写",
            "unexpected reason {}",
            hit.reason
        );

        let plural = d.lookup("apples").unwrap();
        assert_eq!(plural.entry.definition, "n. 苹果");
        assert_eq!(plural.reason, "词形还原");
    }

    #[test]
    fn unseen_word_yields_none() {
        let t = sample();
        let d = Dictionary::open(t.dir.join("testdict.ifo")).unwrap();
        assert!(d.lookup("zzzzz").is_none());
        assert!(d.lookup("").is_none());
    }

    #[test]
    fn anchor_table_is_sparse() {
        let t = sample();
        let d = Dictionary::open(t.dir.join("testdict.ifo")).unwrap();
        assert_eq!(d.entry_count(), 5);
        let max_anchors = d.entry_count() / ANCHOR_STRIDE + 1;
        assert!(
            d.offsets_bytes() <= max_anchors * 4,
            "anchor table must not grow with the record count"
        );
    }

    #[test]
    fn streamed_scan_is_chunk_size_independent() {
        let t = sample();
        let idx = t.dir.join("testdict.idx");

        let (big, big_count) = scan_offsets(&idx, 1 << 20).unwrap();
        let (small, small_count) = scan_offsets(&idx, 7).unwrap();
        let (tiny, tiny_count) = scan_offsets(&idx, 1).unwrap();

        assert_eq!(big, small, "chunk boundary changed the anchors");
        assert_eq!(big, tiny, "byte-at-a-time scan disagrees");
        assert_eq!(big_count, 5);
        assert_eq!(small_count, 5);
        assert_eq!(tiny_count, 5);
        assert_eq!(big[0], 0, "first record must start at offset 0");
    }

    #[test]
    fn lookups_work_across_anchor_blocks() {
        // 100 records force several anchors; every headword must still resolve.
        let entries: Vec<(String, String)> = (0..100)
            .map(|i| (format!("word{i:03}"), format!("def {i}")))
            .collect();
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(w, d)| (w.as_str(), d.as_str()))
            .collect();
        let t = build_temp(&refs);
        let d = Dictionary::open(t.dir.join("testdict.ifo")).unwrap();
        assert_eq!(d.entry_count(), 100);
        for (word, def) in &entries {
            let hit = d.lookup_exact(word).unwrap_or_else(|| panic!("miss {word}"));
            assert_eq!(&hit.definition, def);
        }
        assert!(d.lookup_exact("word999").is_none());
        assert!(d.lookup_exact("wordaa0").is_none());
    }
}
