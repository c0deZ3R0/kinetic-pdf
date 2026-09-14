//! Turning a drag across a page's characters into highlight bands and quoted
//! text, and recovering the text under bands loaded from a file.
//!
//! The browser version got selection for free from the DOM. Here the page's
//! text layer is pdfium's character list, and a selection is a range of
//! indices into it -- which is also why highlights snap to the text rather
//! than to wherever the mouse happened to be.

use std::cmp::Ordering;
use std::ops::Range;

use crate::model::{PdfBox, TextChar};

/// The caret position (a gap between characters) nearest a point in PDF user
/// space. Being on the same line matters far more than horizontal distance,
/// so a drag that starts in the margin still starts at the line beside it.
pub fn caret_at(chars: &[TextChar], x: f32, y: f32) -> Option<usize> {
    let mut best: Option<(f32, usize, PdfBox)> = None;
    for (i, c) in chars.iter().enumerate() {
        let Some(b) = c.bounds else { continue };
        let dy = if y > b.top { y - b.top } else if y < b.bottom { b.bottom - y } else { 0.0 };
        let dx = if x > b.right { x - b.right } else if x < b.left { b.left - x } else { 0.0 };
        let score = dy * 4.0 + dx;
        if best.is_none_or(|(s, _, _)| score < s) {
            best = Some((score, i, b));
        }
    }
    best.map(|(_, i, b)| if x < (b.left + b.right) / 2.0 { i } else { i + 1 })
}

fn is_gap(c: char) -> bool {
    c.is_whitespace() || c.is_control()
}

/// The text around a match, for listing it: up to `before_len` characters
/// before and `after_len` after, cut back to a whole word and marked with "…"
/// wherever the page's text carries on. Where the match meets its neighbours
/// with whitespace between, that is kept as a single space, so the three parts
/// read correctly shown back to back.
pub fn context(chars: &[TextChar], range: Range<usize>, before_len: usize, after_len: usize) -> (String, String, String) {
    let range = clamp(chars, range);
    let matched = text(chars, range.clone());

    // A generous window of raw characters, since collapsing whitespace shrinks it.
    let window_start = range.start.saturating_sub(before_len * 3);
    let mut before = text(chars, window_start..range.start);
    let mut cut = window_start > 0;
    if before.chars().count() > before_len {
        let tail: String = before.chars().skip(before.chars().count() - before_len).collect();
        before = match tail.find(' ') {
            Some(space) => tail[space + 1..].to_owned(),
            None => tail,
        };
        cut = true;
    }
    if cut && !before.is_empty() {
        before.insert(0, '…');
    }
    if !before.is_empty() && range.start > 0 && is_gap(chars[range.start - 1].ch) {
        before.push(' ');
    }

    let window_end = (range.end + after_len * 3).min(chars.len());
    let mut after = text(chars, range.end..window_end);
    let mut cut = window_end < chars.len();
    if after.chars().count() > after_len {
        let head: String = after.chars().take(after_len).collect();
        after = match head.rfind(' ') {
            Some(space) => head[..space].to_owned(),
            None => head,
        };
        cut = true;
    }
    if cut && !after.is_empty() {
        after.push('…');
    }
    if !after.is_empty() && range.end < chars.len() && is_gap(chars[range.end].ch) {
        after.insert(0, ' ');
    }

    (before, matched, after)
}

#[cfg(test)]
mod context_tests {
    use super::*;

    fn page(s: &str) -> Vec<TextChar> {
        s.chars()
            .enumerate()
            .map(|(i, ch)| TextChar {
                ch,
                bounds: Some(PdfBox { left: i as f32, bottom: 0.0, right: i as f32 + 1.0, top: 10.0 }),
            })
            .collect()
    }

    #[test]
    fn short_context_is_kept_whole_with_its_spaces() {
        let chars = page("the quick brown fox jumps");
        let (before, matched, after) = context(&chars, 10..15, 40, 40);
        assert_eq!((before.as_str(), matched.as_str(), after.as_str()), ("the quick ", "brown", " fox jumps"));
    }

    #[test]
    fn long_context_is_cut_at_a_word_and_marked() {
        let chars = page("the quick brown fox jumps");
        let (before, _, after) = context(&chars, 10..15, 6, 6);
        assert_eq!(before, "…quick ");
        assert_eq!(after, " fox…");
    }

    #[test]
    fn match_inside_a_word_gets_no_invented_space() {
        let chars = page("highlight");
        let (before, matched, after) = context(&chars, 4..6, 40, 40);
        assert_eq!((before.as_str(), matched.as_str(), after.as_str()), ("high", "li", "ght"));
    }
}

fn clamp(chars: &[TextChar], range: Range<usize>) -> Range<usize> {
    let end = range.end.min(chars.len());
    range.start.min(end)..end
}

/// One band per line for a range of characters.
pub fn bands(chars: &[TextChar], range: Range<usize>) -> Vec<PdfBox> {
    bands_of(chars, &[range])
}

/// One band per line for several ranges of characters together, as a box
/// selection gives.
pub fn bands_of(chars: &[TextChar], ranges: &[Range<usize>]) -> Vec<PdfBox> {
    let boxes: Vec<PdfBox> = ranges
        .iter()
        .flat_map(|range| chars[clamp(chars, range.clone())].iter())
        // Spaces would stretch a band past the last word on a line; the gap
        // tolerance in merge_boxes bridges the space between words anyway.
        .filter(|c| !c.ch.is_whitespace())
        .filter_map(|c| c.bounds)
        .filter(|b| !b.is_empty())
        .collect();
    merge_boxes(&boxes)
}

/// The characters whose centres fall inside `area`, in PDF user space, as runs
/// of consecutive characters. A line break has no box, so each line gives its
/// own run, and a box drawn over one column of a table picks out that column
/// line by line, leaving the columns beside it alone.
pub fn in_box(chars: &[TextChar], area: &PdfBox) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for (i, c) in chars.iter().enumerate() {
        let inside = c.bounds.is_some_and(|b| {
            let (x, y) = b.center();
            area.contains(x, y)
        });
        if inside {
            match runs.last_mut() {
                Some(run) if run.end == i => run.end = i + 1,
                _ => runs.push(i..i + 1),
            }
        }
    }
    runs
}

/// The text of a range of characters for the clipboard: line breaks kept, and
/// other runs of whitespace collapsed to single spaces.
pub fn copy_text(chars: &[TextChar], range: Range<usize>) -> String {
    let raw: String = chars[clamp(chars, range)].iter().map(|c| c.ch).collect();
    raw.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod box_tests {
    use super::*;

    /// Lines of text, a character every 10 points across and a line every 20
    /// points down, each ending in a line break with no box, as pdfium gives.
    fn lines(rows: &[&str]) -> Vec<TextChar> {
        let mut chars = Vec::new();
        for (row, line) in rows.iter().enumerate() {
            let top = 700.0 - 20.0 * row as f32;
            for (i, ch) in line.chars().enumerate() {
                let left = 50.0 + 10.0 * i as f32;
                chars.push(TextChar { ch, bounds: Some(PdfBox { left, bottom: top - 12.0, right: left + 8.0, top }) });
            }
            chars.push(TextChar { ch: '\r', bounds: None });
            chars.push(TextChar { ch: '\n', bounds: None });
        }
        chars
    }

    #[test]
    fn a_box_over_a_column_selects_that_column_line_by_line() {
        let chars = lines(&["name  qty", "bolt  12", "nut   400"]);
        let column = PdfBox { left: 105.0, bottom: 640.0, right: 150.0, top: 700.0 };
        let runs = in_box(&chars, &column);
        let texts: Vec<String> = runs.iter().map(|run| copy_text(&chars, run.clone())).collect();
        assert_eq!(texts, ["qty", "12", "400"]);
        assert_eq!(bands_of(&chars, &runs).len(), 3, "one band per line");

        // Shorter, it stops at the lines it reaches.
        let top_two = PdfBox { bottom: 660.0, ..column };
        let texts: Vec<String> = in_box(&chars, &top_two).iter().map(|run| copy_text(&chars, run.clone())).collect();
        assert_eq!(texts, ["qty", "12"]);
    }

    #[test]
    fn copied_text_keeps_its_line_breaks() {
        let chars = lines(&["name  qty", "bolt  12"]);
        assert_eq!(copy_text(&chars, 0..chars.len()), "name qty\nbolt 12");
        assert_eq!(text(&chars, 0..chars.len()), "name qty bolt 12", "quotes stay on one line");
    }
}

/// Character boxes arrive one per glyph. Group them into lines and join the
/// ones that sit next to each other, which gives one clean band per line --
/// but keeps two columns on the same baseline apart.
pub fn merge_boxes(boxes: &[PdfBox]) -> Vec<PdfBox> {
    struct Line {
        bottom: f32,
        top: f32,
        parts: Vec<PdfBox>,
    }

    let by_f32 = |a: f32, b: f32| a.partial_cmp(&b).unwrap_or(Ordering::Equal);
    let mut sorted = boxes.to_vec();
    sorted.sort_by(|a, b| by_f32(b.top, a.top).then(by_f32(a.left, b.left)));

    let mut lines: Vec<Line> = Vec::new();
    for r in sorted {
        let mid = (r.top + r.bottom) / 2.0;
        let height = r.height();
        let line = lines.iter_mut().find(|l| {
            ((l.top + l.bottom) / 2.0 - mid).abs() < (l.top - l.bottom).min(height) * 0.6
        });
        match line {
            Some(l) => {
                l.top = l.top.max(r.top);
                l.bottom = l.bottom.min(r.bottom);
                l.parts.push(r);
            }
            None => lines.push(Line { bottom: r.bottom, top: r.top, parts: vec![r] }),
        }
    }

    let mut merged = Vec::new();
    for mut line in lines {
        let gap = (line.top - line.bottom) * 0.6;
        line.parts.sort_by(|a, b| by_f32(a.left, b.left));
        let mut run: Option<PdfBox> = None;
        for r in line.parts {
            if let Some(cur) = run.as_mut() {
                if r.left - cur.right <= gap {
                    cur.right = cur.right.max(r.right);
                    continue;
                }
            }
            if let Some(done) = run.take() {
                merged.push(done);
            }
            run = Some(PdfBox { left: r.left, bottom: line.bottom, right: r.right, top: line.top });
        }
        if let Some(done) = run {
            merged.push(done);
        }
    }
    merged
}

/// The text of a range of characters, with runs of whitespace (including the
/// line breaks pdfium inserts) collapsed to single spaces.
pub fn text(chars: &[TextChar], range: Range<usize>) -> String {
    let raw: String = chars[clamp(chars, range)]
        .iter()
        .map(|c| if c.ch.is_control() { ' ' } else { c.ch })
        .collect();
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// For a highlight that came from the file rather than from a live drag,
/// recover exactly the text it covers: for each band, the first and last
/// character whose centre falls inside it, and everything between. That is
/// precisely what dragging across that span would have quoted -- highlighting
/// "high" in "highlight" quotes "high", not the whole word.
pub fn text_in_quads(chars: &[TextChar], quads: &[PdfBox]) -> String {
    let mut lines = Vec::new();
    for band in quads {
        let mut first = None;
        let mut last = None;
        for (i, c) in chars.iter().enumerate() {
            let Some(b) = c.bounds else { continue };
            let (cx, cy) = b.center();
            if band.contains(cx, cy) {
                first.get_or_insert(i);
                last = Some(i);
            }
        }
        if let (Some(f), Some(l)) = (first, last) {
            lines.push(text(chars, f..l + 1));
        }
    }
    lines.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A query lowercased, trimmed, and with runs of whitespace collapsed -- the
/// form `find` compares against. Empty means there is nothing to search for.
pub fn normalize_query(query: &str) -> Vec<char> {
    query.split_whitespace().collect::<Vec<_>>().join(" ").chars().map(fold).collect()
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Every case-insensitive occurrence of `query` on a page, as character
/// ranges. A space in the query matches any run of whitespace in the text,
/// including the line breaks pdfium inserts, so a phrase that wraps onto the
/// next line is still found. Matches don't overlap.
pub fn find(chars: &[TextChar], query: &str) -> Vec<Range<usize>> {
    let needle = normalize_query(query);
    if needle.is_empty() {
        return Vec::new();
    }

    // The page collapsed the same way, remembering which character each entry
    // came from.
    let mut hay: Vec<(char, usize)> = Vec::with_capacity(chars.len());
    for (i, c) in chars.iter().enumerate() {
        if c.ch.is_whitespace() || c.ch.is_control() {
            if hay.last().is_some_and(|(prev, _)| *prev != ' ') {
                hay.push((' ', i));
            }
        } else {
            hay.push((fold(c.ch), i));
        }
    }

    let mut found = Vec::new();
    let mut at = 0;
    while at + needle.len() <= hay.len() {
        if hay[at..at + needle.len()].iter().zip(&needle).all(|((h, _), n)| h == n) {
            let last = hay[at + needle.len() - 1].1;
            found.push(hay[at].1..last + 1);
            at += needle.len();
        } else {
            at += 1;
        }
    }
    found
}

#[cfg(test)]
mod find_tests {
    use super::*;

    /// One line of text; control characters get no bounds, as from pdfium.
    fn page(s: &str) -> Vec<TextChar> {
        s.chars()
            .enumerate()
            .map(|(i, ch)| TextChar {
                ch,
                bounds: (!ch.is_control()).then(|| PdfBox {
                    left: i as f32 * 6.0,
                    bottom: 0.0,
                    right: (i + 1) as f32 * 6.0,
                    top: 12.0,
                }),
            })
            .collect()
    }

    #[test]
    fn finds_every_occurrence_ignoring_case() {
        assert_eq!(find(&page("Cat, cat and CAT"), "cat"), vec![0..3, 5..8, 13..16]);
    }

    #[test]
    fn phrase_matches_across_a_line_break() {
        let chars = page("the quick\r\nbrown fox");
        let found = find(&chars, "quick  brown");
        assert_eq!(found, vec![4..16]);
        assert_eq!(text(&chars, found[0].clone()), "quick brown");
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(find(&page("aaaa"), "aa"), vec![0..2, 2..4]);
    }

    #[test]
    fn blank_query_finds_nothing() {
        assert!(find(&page("anything"), "   ").is_empty());
        assert!(normalize_query(" \t ").is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of monospaced text: each character `w` wide, the line's
    /// baseline box running from `y` to `y + 12`.
    fn line(s: &str, x: f32, y: f32, w: f32) -> Vec<TextChar> {
        s.chars()
            .enumerate()
            .map(|(i, ch)| TextChar {
                ch,
                bounds: Some(PdfBox {
                    left: x + i as f32 * w,
                    bottom: y,
                    right: x + (i as f32 + 1.0) * w,
                    top: y + 12.0,
                }),
            })
            .collect()
    }

    /// Two lines joined the way pdfium reports them: a synthesised line break
    /// with no bounds in between.
    fn two_lines() -> Vec<TextChar> {
        let mut chars = line("highlight this", 40.0, 700.0, 6.0);
        chars.push(TextChar { ch: '\r', bounds: None });
        chars.push(TextChar { ch: '\n', bounds: None });
        chars.extend(line("second line", 40.0, 680.0, 6.0));
        chars
    }

    #[test]
    fn caret_lands_on_the_nearer_side_of_a_character() {
        let chars = line("abc", 0.0, 0.0, 10.0);
        assert_eq!(caret_at(&chars, 2.0, 6.0), Some(0));
        assert_eq!(caret_at(&chars, 8.0, 6.0), Some(1));
        assert_eq!(caret_at(&chars, 29.0, 6.0), Some(3));
    }

    #[test]
    fn caret_from_the_margin_stays_on_that_line() {
        let chars = two_lines();
        // Left of the second line, level with it.
        let caret = caret_at(&chars, 0.0, 685.0).unwrap();
        assert_eq!(chars[caret].ch, 's');
    }

    #[test]
    fn one_line_selection_is_one_band() {
        let chars = line("hello world", 10.0, 100.0, 6.0);
        let b = bands(&chars, 0..chars.len());
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].left, 10.0);
        assert_eq!(b[0].right, 10.0 + 11.0 * 6.0);
    }

    #[test]
    fn selection_across_a_line_break_gives_a_band_per_line() {
        let chars = two_lines();
        let b = bands(&chars, 10..chars.len());
        assert_eq!(b.len(), 2);
        assert!(b[0].top > b[1].top);
        assert_eq!(text(&chars, 10..chars.len()), "this second line");
    }

    #[test]
    fn columns_on_one_baseline_stay_separate() {
        let mut chars = line("left", 40.0, 500.0, 6.0);
        chars.extend(line("right", 300.0, 500.0, 6.0));
        assert_eq!(bands(&chars, 0..chars.len()).len(), 2);
    }

    #[test]
    fn part_of_a_word_quotes_only_that_part() {
        let chars = two_lines();
        let quads = bands(&chars, 0..4);
        assert_eq!(text(&chars, 0..4), "high");
        assert_eq!(text_in_quads(&chars, &quads), "high");
    }

    #[test]
    fn saved_multi_line_highlight_recovers_its_text() {
        let chars = two_lines();
        // "this" through "second".
        let quads = bands(&chars, 10..22);
        assert_eq!(text_in_quads(&chars, &quads), "this second");
    }
}
