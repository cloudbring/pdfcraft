//! pdfcraft-compare — Compare files: text differences (L4).
//!
//! Both documents' words (in reading order, with page and box) are diffed by craft-diff —
//! the anchored, budgeted sequence diff the Craft Apps share (cloudbring/craft-libs,
//! artcraft-algorithms #53/#54): common edges, bounded Myers, patience anchors and, where a
//! repeated vocabulary leaves nothing unique, the longest common run of repeated words. The
//! edit script is grouped into [`Change`]s — inserted, deleted or replaced runs of words —
//! each with the rectangles to highlight on the old and new pages.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// A word of one document.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// 0-based page.
    pub page: usize,
    /// [x0, y0, x1, y1] in the page's user space.
    pub rect: [f64; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Inserted,
    Deleted,
    Replaced,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Inserted => "Inserted",
            Kind::Deleted => "Deleted",
            Kind::Replaced => "Replaced",
        }
    }
}

/// One side of a change: its words' text, page and rectangles (merged per line).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Side {
    pub text: String,
    /// The page the change is on (for a pure insertion or deletion, where the other side's
    /// words would have been).
    pub page: usize,
    pub rects: Vec<[f64; 4]>,
    /// For an empty side (the other side's words were inserted or deleted): the box of the
    /// word next to where they would be.
    pub near: Option<[f64; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub kind: Kind,
    pub old: Side,
    pub new: Side,
}

/// What the comparison found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Comparison {
    pub changes: Vec<Change>,
    /// Words in each document.
    pub old_words: usize,
    pub new_words: usize,
}

impl Comparison {
    pub fn count(&self, kind: Kind) -> usize {
        self.changes.iter().filter(|c| c.kind == kind).count()
    }

    /// Whether the documents' text is the same.
    pub fn identical(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Merge word boxes on the same page and line into one rectangle per line.
fn rects(words: &[Word]) -> Vec<[f64; 4]> {
    let mut out: Vec<[f64; 4]> = Vec::new();
    for w in words {
        let r = w.rect;
        match out.last_mut() {
            Some(l) if (l[1] - r[1]).abs() < (r[3] - r[1]).max(1.0) * 0.5 && r[0] - l[2] < (r[3] - r[1]).max(4.0) * 2.0 && r[0] >= l[0] - 1.0 => {
                l[0] = l[0].min(r[0]);
                l[1] = l[1].min(r[1]);
                l[2] = l[2].max(r[2]);
                l[3] = l[3].max(r[3]);
            }
            _ => out.push(r),
        }
    }
    out
}

fn side(words: &[Word], neighbour: Option<&Word>) -> Side {
    Side {
        text: words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
        page: words.first().or(neighbour).map_or(0, |w| w.page),
        rects: rects(words),
        near: if words.is_empty() { neighbour.map(|w| w.rect) } else { None },
    }
}

/// Compare two documents' words.
pub fn compare(old: &[Word], new: &[Word]) -> Comparison {
    let a: Vec<&str> = old.iter().map(|w| w.text.as_str()).collect();
    let b: Vec<&str> = new.iter().map(|w| w.text.as_str()).collect();
    let mut changes = Vec::new();
    for op in craft_diff::diff(&a, &b, &craft_diff::Budget::APPS) {
        let (kind, o, n) = match op {
            craft_diff::Op::Equal { .. } => continue,
            craft_diff::Op::Insert { old, new } => (Kind::Inserted, old, new),
            craft_diff::Op::Delete { old, new } => (Kind::Deleted, old, new),
            craft_diff::Op::Replace { old, new } => (Kind::Replaced, old, new),
        };
        // Where a one-sided change sits in the other document: next to the neighbouring word.
        let old_near = old.get(o.end).or_else(|| old.last());
        let new_near = new.get(n.end).or_else(|| new.last());
        changes.push(Change { kind, old: side(&old[o], old_near), new: side(&new[n], new_near) });
    }
    Comparison { changes, old_words: old.len(), new_words: new.len() }
}

/// The compare report's text: a summary, then each change with its pages.
pub fn report(c: &Comparison, old_name: &str, new_name: &str) -> String {
    let mut s = format!(
        "Compare Report\n\nOld file: {old_name}\nNew file: {new_name}\n\n{} changes: {} replaced, {} inserted, {} deleted.\n",
        c.changes.len(),
        c.count(Kind::Replaced),
        c.count(Kind::Inserted),
        c.count(Kind::Deleted)
    );
    if c.identical() {
        s.push_str("\nThe documents' text is identical.\n");
    }
    for (n, ch) in c.changes.iter().enumerate() {
        s.push_str(&format!("\n{}. {} (old page {}, new page {})\n", n + 1, ch.kind.label(), ch.old.page + 1, ch.new.page + 1));
        if !ch.old.text.is_empty() {
            s.push_str(&format!("   Old: {}\n", ch.old.text));
        }
        if !ch.new.text.is_empty() {
            s.push_str(&format!("   New: {}\n", ch.new.text));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str, page: usize) -> Vec<Word> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, t)| Word { text: t.into(), page, rect: [10.0 + 40.0 * i as f64, 700.0, 45.0 + 40.0 * i as f64, 710.0] })
            .collect()
    }

    #[test]
    fn identical_text_has_no_changes() {
        let c = compare(&words("the same words here", 0), &words("the same words here", 0));
        assert!(c.identical());
    }

    #[test]
    fn insertions_deletions_and_replacements() {
        let old = words("The quick brown fox jumps over the lazy dog", 0);
        let new = words("The quick red fox leaps over the very lazy dog today", 0);
        let c = compare(&old, &new);
        let got: Vec<(Kind, &str, &str)> = c.changes.iter().map(|x| (x.kind, x.old.text.as_str(), x.new.text.as_str())).collect();
        assert_eq!(
            got,
            [(Kind::Replaced, "brown", "red"), (Kind::Replaced, "jumps", "leaps"), (Kind::Inserted, "", "very"), (Kind::Inserted, "", "today"),]
        );
        assert_eq!(c.changes[0].new.rects, [[90.0, 700.0, 125.0, 710.0]]);
        assert_eq!(c.count(Kind::Inserted), 2);
    }

    #[test]
    fn deletions_across_pages_keep_their_pages() {
        let mut old = words("alpha beta gamma", 0);
        old.extend(words("delta epsilon", 1));
        let mut new = words("alpha gamma", 0);
        new.extend(words("delta", 1));
        let c = compare(&old, &new);
        let got: Vec<(Kind, usize, &str)> = c.changes.iter().map(|x| (x.kind, x.old.page, x.old.text.as_str())).collect();
        assert_eq!(got, [(Kind::Deleted, 0, "beta"), (Kind::Deleted, 1, "epsilon")]);
        // The deletion's place in the new document: the page of the words around it.
        assert_eq!(c.changes[1].new.page, 1);
        let r = report(&c, "v1.pdf", "v2.pdf");
        assert!(r.contains("2 changes: 0 replaced, 0 inserted, 2 deleted.") && r.contains("Old: epsilon"), "{r}");
    }

    #[test]
    fn adjacent_words_merge_into_one_rectangle_per_line() {
        let old = words("a b c", 0);
        let new = words("x y z", 0);
        let c = compare(&old, &new);
        assert_eq!(c.changes.len(), 1);
        assert_eq!(c.changes[0].new.rects.len(), 1);
    }

    #[test]
    fn long_documents_with_scattered_edits() {
        let text: Vec<String> = (0..20_000).map(|i| format!("w{i}")).collect();
        let old: Vec<Word> = text.iter().map(|t| Word { text: t.clone(), page: 0, rect: [0.0; 4] }).collect();
        let mut new = old.clone();
        for k in [100usize, 5000, 12_000, 19_000] {
            new[k].text = format!("changed{k}");
        }
        new.remove(8000);
        let c = compare(&old, &new);
        assert_eq!(c.changes.len(), 5, "{:?}", c.changes.iter().map(|x| (x.kind, &x.old.text, &x.new.text)).collect::<Vec<_>>());
        assert_eq!(c.count(Kind::Replaced), 4);
    }

    #[test]
    fn very_different_documents_finish() {
        let old: Vec<Word> = (0..30_000).map(|i| Word { text: format!("a{i}"), page: 0, rect: [0.0; 4] }).collect();
        let new: Vec<Word> = (0..30_000).map(|i| Word { text: format!("b{i}"), page: 0, rect: [0.0; 4] }).collect();
        let c = compare(&old, &new);
        assert_eq!(c.changes.len(), 1);
        assert_eq!(c.changes[0].kind, Kind::Replaced);
    }

    /// The diff-quality metric of #54: how many words the report flags as changed.
    fn changed_words(c: &Comparison) -> usize {
        c.changes.iter().map(|ch| ch.old.text.split_whitespace().count() + ch.new.text.split_whitespace().count()).sum()
    }

    #[test]
    fn repetitive_vocabulary_keeps_scattered_edits() {
        // #54: 10,000 words over a 50-word vocabulary, 20 scattered replacements. No word is
        // unique in either document, so patience has no anchor and the whole middle used to
        // collapse into one replacement.
        let vocab: Vec<String> = (0..50).map(|i| format!("term{i}")).collect();
        let old: Vec<Word> = (0..10_000).map(|i| Word { text: vocab[i % 50].clone(), page: 0, rect: [0.0; 4] }).collect();
        let mut new = old.clone();
        for k in 0..20 {
            let at = 199 + 487 * k;
            new[at].text = vocab[(at + 7) % 50].clone();
        }
        let c = compare(&old, &new);
        let changed = changed_words(&c);
        eprintln!("10,000-word pair: {} changes, {changed} changed words", c.changes.len());
        assert!(changed <= 40, "{} changes covering {} words", c.changes.len(), changed);
        assert_eq!(c.count(Kind::Replaced), 20, "{:?}", c.changes.iter().map(|x| (&x.old.text, &x.new.text)).collect::<Vec<_>>());
    }

    #[test]
    fn fixture_changed_word_counts_hold() {
        // #54: the extra anchoring must not enlarge the diff on the existing fixtures.
        let contract = "The contract starts on Monday. Payment is due within thirty days. Signed by both parties.";
        let revised = "The contract starts on Tuesday. Payment is due within thirty days of invoice. Signed by both parties.";
        let across_pages = |text: &[(&str, usize)]| text.iter().flat_map(|(t, p)| words(t, *p)).collect::<Vec<_>>();
        let cases = [
            ("identical", 0, words("the same words here", 0), words("the same words here", 0)),
            (
                "replacements",
                6,
                words("The quick brown fox jumps over the lazy dog", 0),
                words("The quick red fox leaps over the very lazy dog today", 0),
            ),
            ("across pages", 2, across_pages(&[("alpha beta gamma", 0), ("delta epsilon", 1)]), across_pages(&[("alpha gamma", 0), ("delta", 1)])),
            ("one line replaced", 6, words("a b c", 0), words("x y z", 0)),
            ("contract", 6, words(contract, 0), words(revised, 0)),
        ];
        for (name, expected, old, new) in cases {
            let changed = changed_words(&compare(&old, &new));
            assert_eq!(changed, expected, "{name}: {changed} changed words");
        }
        // The two large fixtures, built as in their tests.
        let text: Vec<String> = (0..20_000).map(|i| format!("w{i}")).collect();
        let old: Vec<Word> = text.iter().map(|t| Word { text: t.clone(), page: 0, rect: [0.0; 4] }).collect();
        let mut new = old.clone();
        for k in [100usize, 5000, 12_000, 19_000] {
            new[k].text = format!("changed{k}");
        }
        new.remove(8000);
        assert_eq!(changed_words(&compare(&old, &new)), 9, "scattered edits");
        let old: Vec<Word> = (0..30_000).map(|i| Word { text: format!("a{i}"), page: 0, rect: [0.0; 4] }).collect();
        let new: Vec<Word> = (0..30_000).map(|i| Word { text: format!("b{i}"), page: 0, rect: [0.0; 4] }).collect();
        assert_eq!(changed_words(&compare(&old, &new)), 60_000, "very different");
    }

    #[test]
    fn fifty_thousand_word_pairs_stay_within_budget() {
        // #54: the worst shapes at 50,000 words a side — repeated vocabulary with scattered
        // edits (histogram anchors, then Myers per gap), a block swap of repeated words, and
        // two documents with nothing in common — all finish inside the budget.
        let start = std::time::Instant::now();
        let vocab: Vec<String> = (0..50).map(|i| format!("w{i}")).collect();
        let old: Vec<Word> = (0..50_000).map(|i| Word { text: vocab[i % 50].clone(), page: 0, rect: [0.0; 4] }).collect();
        let mut new = old.clone();
        for k in 0..100 {
            let at = 101 + 431 * k;
            new[at].text = vocab[(at + 11) % 50].clone();
        }
        let c = compare(&old, &new);
        let changed = changed_words(&c);
        eprintln!("50,000-word pair: {} changes, {changed} changed words", c.changes.len());
        assert!(changed <= 200, "{} changes covering {} words", c.changes.len(), changed);

        let block = |first: &str, second: &str| {
            (0..50_000).map(|i| Word { text: (if i < 25_000 { first } else { second }).into(), page: 0, rect: [0.0; 4] }).collect::<Vec<_>>()
        };
        let swapped = compare(&block("alpha", "beta"), &block("beta", "alpha"));
        // A block swap of repeated words has no monotone matches beyond one half: 50,000.
        assert_eq!(changed_words(&swapped), 50_000);

        let old: Vec<Word> = (0..50_000).map(|i| Word { text: format!("x{i}"), page: 0, rect: [0.0; 4] }).collect();
        let new: Vec<Word> = (0..50_000).map(|i| Word { text: format!("y{i}"), page: 0, rect: [0.0; 4] }).collect();
        assert_eq!(compare(&old, &new).changes.len(), 1);

        let elapsed = start.elapsed();
        eprintln!("50,000-word pairs: {elapsed:?}");
        assert!(elapsed < std::time::Duration::from_secs(30), "{elapsed:?}");
    }
}

/// Visual compare: the regions where two renderings of a page differ, as pixel boxes
/// `[x0, y0, x1, y1]` (y down) in `a`'s image. Both images are RGBA; `b` is sampled at the
/// same relative position when sizes differ. Pixels count as different when a channel differs
/// by more than `tolerance`; differing cells of a coarse grid are joined into regions.
pub fn visual_regions(a: (&[u8], u32, u32), b: (&[u8], u32, u32), tolerance: u8) -> Vec<[u32; 4]> {
    let (pa, wa, ha) = a;
    let (pb, wb, hb) = b;
    if wa == 0 || ha == 0 || wb == 0 || hb == 0 {
        return Vec::new();
    }
    const CELL: u32 = 8;
    let (gw, gh) = (wa.div_ceil(CELL), ha.div_ceil(CELL));
    let mut grid = vec![false; (gw * gh) as usize];
    for y in 0..ha {
        let yb = (y as u64 * hb as u64 / ha as u64) as u32;
        for x in 0..wa {
            let xb = (x as u64 * wb as u64 / wa as u64) as u32;
            let ia = ((y * wa + x) * 4) as usize;
            let ib = ((yb * wb + xb) * 4) as usize;
            let (Some(ca), Some(cb)) = (pa.get(ia..ia + 3), pb.get(ib..ib + 3)) else { continue };
            if ca.iter().zip(cb).any(|(p, q)| p.abs_diff(*q) > tolerance) {
                grid[((y / CELL) * gw + x / CELL) as usize] = true;
            }
        }
    }
    // Connected cells (8-neighbourhood, with a one-cell gap bridged) become one region.
    let mut seen = vec![false; grid.len()];
    let mut out = Vec::new();
    for start in 0..grid.len() {
        if !grid[start] || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(c) = stack.pop() {
            let (cx, cy) = (c as u32 % gw, c as u32 / gw);
            x0 = x0.min(cx);
            y0 = y0.min(cy);
            x1 = x1.max(cx);
            y1 = y1.max(cy);
            for dy in -2i64..=2 {
                for dx in -2i64..=2 {
                    let (nx, ny) = (cx as i64 + dx, cy as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= gw as i64 || ny >= gh as i64 {
                        continue;
                    }
                    let n = (ny as u32 * gw + nx as u32) as usize;
                    if grid[n] && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        out.push([x0 * CELL, y0 * CELL, ((x1 + 1) * CELL).min(wa), ((y1 + 1) * CELL).min(ha)]);
    }
    out
}

#[cfg(test)]
mod visual_tests {
    use super::*;

    fn image(w: u32, h: u32, boxes: &[[u32; 4]]) -> Vec<u8> {
        let mut px = vec![255u8; (w * h * 4) as usize];
        for b in boxes {
            for y in b[1]..b[3] {
                for x in b[0]..b[2] {
                    let i = ((y * w + x) * 4) as usize;
                    px[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        px
    }

    #[test]
    fn finds_changed_regions() {
        let a = image(200, 200, &[[10, 10, 50, 20]]);
        let b = image(200, 200, &[[10, 10, 50, 20], [120, 150, 160, 170]]);
        assert!(visual_regions((&a, 200, 200), (&a, 200, 200), 16).is_empty());
        let r = visual_regions((&a, 200, 200), (&b, 200, 200), 16);
        assert_eq!(r, [[120, 144, 160, 176]]);
        // Two nearby marks join; distant ones don't.
        let c = image(200, 200, &[[10, 10, 50, 20], [100, 100, 104, 104], [110, 100, 114, 104], [10, 180, 14, 184]]);
        assert_eq!(visual_regions((&a, 200, 200), (&c, 200, 200), 16).len(), 2);
    }
}
