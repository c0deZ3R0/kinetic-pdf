//! A text box's text set in lines: where each word goes, in which face and
//! at what size. The screen and the PDF both draw from this, so they break
//! lines in the same places and put every word in the same spot.
//!
//! Lines break between words, filling each line as far as it goes. A word
//! wider than the box stays whole on a line of its own. With the box's `fit`
//! set, the text is made as big as fits -- all of it together, its sizes
//! kept in proportion -- so a few words fill the box, and more shrink it.

use std::sync::Arc;

use markup_model::markup::Rgb;
use markup_model::{HAlign, TextBox, VAlign};

use crate::fonts::{Catalogue, Face};

/// Text set in a box.
pub struct Laid {
    /// What its sizes were multiplied by to fit: 1 if they weren't.
    pub scale: f32,
    pub lines: Vec<Line>,
    /// How tall the text is, in points.
    pub height: f32,
}

/// One line of text.
pub struct Line {
    /// Its baseline, in points down from the top of the box's inside.
    pub baseline: f32,
    pub words: Vec<Word>,
}

/// A piece of a line in one face and size -- a word, with the space after
/// it -- and where it starts along the line.
pub struct Word {
    pub face: Arc<Face>,
    /// Points, shrunk to fit.
    pub size: f32,
    /// Points across from the left of the box's inside.
    pub x: f32,
    pub text: String,
    /// Each character's glyph, and how far it moves the pen, in points.
    pub glyphs: Vec<(u16, f32)>,
    /// How wide it is without its space after, in points.
    pub width: f32,
    pub colour: Rgb,
    pub underline: bool,
    /// Bold or italic asked for that the font hasn't got a face for, so
    /// what draws it makes it look so.
    pub fake_bold: bool,
    pub fake_italic: bool,
}

/// A piece of a paragraph before it's put on a line.
struct Piece {
    face: Arc<Face>,
    size: f32,
    text: String,
    /// Wide without its trailing space, and with it.
    width: f32,
    advance: f32,
    colour: Rgb,
    underline: bool,
    fake_bold: bool,
    fake_italic: bool,
    /// Whether a line may break after it.
    breaks: bool,
}

/// `text` set in a box whose inside is `width` by `height` points, in fonts
/// from `fonts`.
pub fn layout(text: &TextBox, width: f32, height: f32, fonts: &Catalogue) -> Laid {
    let width = width.max(1.0);
    let whole = set(text, width, 1.0, fonts);
    // Nothing typed yet has no size to fill the box with.
    let typed = text.paragraphs.iter().flat_map(|p| &p.runs).any(|r| !r.text.trim().is_empty());
    if !text.fit || !typed {
        return place(whole, text.valign, height);
    }
    let fits = |scale: f32| {
        let laid = if scale == 1.0 { None } else { Some(set(text, width, scale, fonts)) };
        laid.as_ref().unwrap_or(&whole).height <= height + 0.01 && widest_word(text, scale, fonts) <= width + 0.01
    };
    // Between a scale it fits at and one it doesn't: from 1 up by doubling
    // when it fits already, so a few words fill the box, and down otherwise.
    let (mut fitting, mut over) = if fits(1.0) {
        let (mut fitting, mut over) = (1.0f32, 2.0f32);
        while over < MOST && fits(over) {
            (fitting, over) = (over, over * 2.0);
        }
        (fitting, over)
    } else {
        (LEAST, 1.0)
    };
    if over >= MOST && fits(MOST) {
        return place(set(text, width, MOST, fonts), text.valign, height);
    }
    // The largest scale it fits at, found by halving: a dozen steps pin it
    // to within a fraction of a percent.
    for _ in 0..14 {
        let middle = (fitting + over) / 2.0;
        if fits(middle) {
            fitting = middle;
        } else {
            over = middle;
        }
    }
    place(set(text, width, fitting, fonts), text.valign, height)
}

/// How far fitting text to its box scales it, down and up.
const LEAST: f32 = 0.02;
const MOST: f32 = 256.0;

/// The widest single word at `scale`, which no line break can help.
fn widest_word(text: &TextBox, scale: f32, fonts: &Catalogue) -> f32 {
    text.paragraphs.iter().flat_map(|p| pieces(p, scale, fonts)).map(|p| p.width).fold(0.0, f32::max)
}

/// Moves the lines down the box as `valign` says.
fn place(mut laid: Laid, valign: VAlign, height: f32) -> Laid {
    let room = (height - laid.height).max(0.0);
    let down = match valign {
        VAlign::Top => 0.0,
        VAlign::Middle => room / 2.0,
        VAlign::Bottom => room,
    };
    for line in &mut laid.lines {
        line.baseline += down;
    }
    laid
}

/// A paragraph cut where lines may break: after the spaces that follow a
/// word, or where the format changes.
fn pieces(paragraph: &markup_model::Paragraph, scale: f32, fonts: &Catalogue) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for run in &paragraph.runs {
        let f = &run.format;
        let Some(face) = fonts.face(&f.font, f.bold, f.italic) else { continue };
        let size = f.size as f32 * scale;
        let (fake_bold, fake_italic) = (f.bold && !face.entry.bold, f.italic && !face.entry.italic);
        let mut word = String::new();
        let mut flush = |word: &mut String, breaks: bool| {
            if word.is_empty() {
                return;
            }
            let trimmed = word.trim_end_matches(' ');
            out.push(Piece {
                face: Arc::clone(&face),
                size,
                width: face.width(trimmed, size),
                advance: face.width(word, size),
                text: std::mem::take(word),
                colour: f.colour,
                underline: f.underline,
                fake_bold,
                fake_italic,
                breaks,
            });
        };
        let mut chars = run.text.chars().peekable();
        while let Some(ch) = chars.next() {
            word.push(ch);
            if ch == ' ' && chars.peek().is_some_and(|next| *next != ' ') {
                flush(&mut word, true);
            }
        }
        // A run ends where the format changes, which isn't a place to break
        // unless it ends in a space.
        let ends_in_space = word.ends_with(' ');
        flush(&mut word, ends_in_space);
    }
    out
}

/// `text` set at `scale` in lines `width` points wide, from the top.
fn set(text: &TextBox, width: f32, scale: f32, fonts: &Catalogue) -> Laid {
    let mut lines: Vec<Line> = Vec::new();
    let mut top = 0.0f32;
    for paragraph in &text.paragraphs {
        let pieces = pieces(paragraph, scale, fonts);
        if pieces.is_empty() {
            // An empty paragraph is an empty line, as tall as its format's.
            let format = paragraph.runs.first().map(|r| r.format.clone()).unwrap_or_else(|| text.format());
            if let Some(face) = fonts.face(&format.font, format.bold, format.italic) {
                let size = format.size as f32 * scale;
                top += (face.ascent - face.descent + face.line_gap) * size;
            }
            continue;
        }
        // Words gathered into lines: each takes pieces while they fit.
        let mut start = 0;
        while start < pieces.len() {
            let mut end = start;
            let mut used = 0.0f32;
            let mut last_break = None;
            while end < pieces.len() {
                // The pieces up to the next place a line may break go
                // together: a word split across formats stays whole.
                let mut through = end;
                while through < pieces.len() - 1 && !pieces[through].breaks {
                    through += 1;
                }
                let group: f32 = pieces[end..through].iter().map(|p| p.advance).sum::<f32>() + pieces[through].width;
                if used + group > width && last_break.is_some() {
                    break;
                }
                used += pieces[end..=through].iter().map(|p| p.advance).sum::<f32>();
                end = through + 1;
                last_break = Some(end);
            }
            let end = last_break.unwrap_or(end).max(start + 1);
            let line = &pieces[start..end];
            let ascent = line.iter().map(|p| p.face.ascent * p.size).fold(0.0, f32::max);
            let descent = line.iter().map(|p| -p.face.descent * p.size).fold(0.0, f32::max);
            let gap = line.iter().map(|p| p.face.line_gap * p.size).fold(0.0, f32::max);
            let last = line.len() - 1;
            let used: f32 = line[..last].iter().map(|p| p.advance).sum::<f32>() + line[last].width;
            let spare = (width - used).max(0.0);
            let final_line = end == pieces.len();
            // Justified, the spare room goes between the words, except on a
            // paragraph's last line.
            let (mut x, between) = match paragraph.align {
                HAlign::Left => (0.0, 0.0),
                HAlign::Centre => (spare / 2.0, 0.0),
                HAlign::Right => (spare, 0.0),
                HAlign::Justify if final_line || last == 0 => (0.0, 0.0),
                HAlign::Justify => (0.0, spare / last as f32),
            };
            let baseline = top + ascent;
            let mut words = Vec::with_capacity(line.len());
            for piece in line {
                let glyphs = piece.text.chars().map(|ch| {
                    let (glyph, advance) = piece.face.glyph(ch);
                    (glyph, advance * piece.size)
                });
                words.push(Word {
                    face: Arc::clone(&piece.face),
                    size: piece.size,
                    x,
                    text: piece.text.clone(),
                    glyphs: glyphs.collect(),
                    width: piece.width,
                    colour: piece.colour,
                    underline: piece.underline,
                    fake_bold: piece.fake_bold,
                    fake_italic: piece.fake_italic,
                });
                x += piece.advance + if piece.breaks { between } else { 0.0 };
            }
            lines.push(Line { baseline, words });
            top += ascent + descent + gap;
            start = end;
        }
    }
    Laid { scale, lines, height: top }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::catalogue;
    use markup_model::RunFormat;

    fn arial(size: f64) -> RunFormat {
        RunFormat { font: "Arial".into(), size, ..RunFormat::default() }
    }

    fn lines_of(laid: &Laid) -> Vec<String> {
        laid.lines.iter().map(|l| l.words.iter().map(|w| w.text.as_str()).collect::<String>().trim_end().to_owned()).collect()
    }

    #[test]
    fn lines_break_between_words_and_paragraphs_start_new_lines() {
        if !catalogue().has("Arial") {
            return;
        }
        let mut text = TextBox::plain("Existing kerb and channel to be removed\nNew", &arial(10.0), HAlign::Left);
        text.fit = false;
        let laid = layout(&text, 90.0, 500.0, catalogue());
        let lines = lines_of(&laid);
        assert!(lines.len() >= 3, "{lines:?}");
        assert_eq!(lines.last().map(String::as_str), Some("New"), "a paragraph starts a line");
        assert!(laid.lines.iter().all(|l| l.words.last().map_or(true, |w| w.x + w.width <= 90.01)), "every line within the box");
        assert!(laid.lines.windows(2).all(|pair| pair[1].baseline > pair[0].baseline));
        assert_eq!(laid.scale, 1.0);
    }

    #[test]
    fn text_too_big_shrinks_to_fit_and_alignment_moves_it_across_and_down() {
        if !catalogue().has("Arial") {
            return;
        }
        let text = TextBox::plain("A great deal of text that will never fit in so small a box at this size", &arial(24.0), HAlign::Centre);
        let laid = layout(&text, 80.0, 40.0, catalogue());
        assert!(laid.scale < 1.0, "it shrank");
        assert!(laid.height <= 40.01, "and fits: {}", laid.height);

        let mut short = TextBox::plain("Hi", &arial(10.0), HAlign::Right);
        short.valign = VAlign::Bottom;
        short.fit = false;
        let laid = layout(&short, 100.0, 100.0, catalogue());
        let word = &laid.lines[0].words[0];
        assert!((word.x + word.width - 100.0).abs() < 0.01, "right-aligned: {} + {}", word.x, word.width);
        assert!(laid.lines[0].baseline > 80.0, "at the bottom: {}", laid.lines[0].baseline);
    }

    #[test]
    fn a_few_words_grow_to_fill_the_box_and_more_shrink_them() {
        if !catalogue().has("Arial") {
            return;
        }
        let few = TextBox::plain("Kerb", &arial(10.0), HAlign::Left);
        let laid = layout(&few, 200.0, 100.0, catalogue());
        assert!(laid.scale > 3.0, "grown: {}", laid.scale);
        let word = &laid.lines[0].words[0];
        let filled = (200.0 - word.width).min(100.0 - laid.height);
        assert!((-0.05..1.0).contains(&filled), "to one side or the other of the box: {} wide, {} tall", word.width, laid.height);

        let more = TextBox::plain("Kerb and channel to be removed and replaced", &arial(10.0), HAlign::Left);
        let smaller = layout(&more, 200.0, 100.0, catalogue()).scale;
        assert!(smaller < laid.scale, "more words, smaller: {smaller}");
        assert_eq!(layout(&TextBox::plain("", &arial(10.0), HAlign::Left), 200.0, 100.0, catalogue()).scale, 1.0, "nothing typed is as set");
    }

    #[test]
    fn justified_lines_reach_both_edges_but_the_last() {
        if !catalogue().has("Arial") {
            return;
        }
        let mut text = TextBox::plain("one two three four five six seven eight nine ten", &arial(10.0), HAlign::Justify);
        text.fit = false;
        let laid = layout(&text, 100.0, 500.0, catalogue());
        let first = &laid.lines[0];
        let end = first.words.last().map(|w| w.x + w.width).unwrap();
        assert!((end - 100.0).abs() < 0.05, "the first line reaches the right edge: {end}");
        let last = laid.lines.last().unwrap();
        assert!(last.words.last().map(|w| w.x + w.width).unwrap() < 99.0, "the last doesn't");
    }
}
