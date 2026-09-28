//! What a text box says, and how: paragraphs of runs of text, each run in a
//! font, size and colour of its own, laid out in the box by the box's own
//! settings -- alignment, padding, and whether the text shrinks to fit.
//!
//! The box itself is the markup's geometry: its four corners, the text's
//! bottom left first and on round anticlockwise, as a clip's are. Its border
//! and background are the markup's `Style`: the line, and the fill.

use serde::{Deserialize, Serialize};

use crate::geom::Pt;
use crate::markup::Rgb;

/// A text box's words and how they're set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextBox {
    pub paragraphs: Vec<Paragraph>,
    /// Where the text sits up and down the box.
    #[serde(default)]
    pub valign: VAlign,
    /// Points between the box's edge and the text.
    #[serde(default = "default_padding")]
    pub padding: f64,
    /// Whether text too big for the box is made smaller until it fits,
    /// rather than running out of it.
    #[serde(default = "yes")]
    pub fit: bool,
    /// Where the box's arrow points, in user space, if it has one. The arrow
    /// runs from the nearest side of the box; moving the box leaves it
    /// pointing at the same place.
    #[serde(default)]
    pub callout: Option<Pt>,
}

/// One paragraph: runs of text in one line of flow, aligned as a whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paragraph {
    pub runs: Vec<Run>,
    #[serde(default)]
    pub align: HAlign,
}

/// Text in one format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    pub format: RunFormat,
}

/// How a run of text looks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunFormat {
    /// The font family, by the name it's installed under.
    pub font: String,
    /// Points.
    pub size: f64,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    pub colour: Rgb,
}

impl Default for RunFormat {
    fn default() -> Self {
        RunFormat { font: "Arial".to_owned(), size: 12.0, bold: false, italic: false, underline: false, colour: [0.0, 0.0, 0.0] }
    }
}

impl RunFormat {
    /// This format with what changed from `before` to `after` changed in it
    /// too, and the rest left as it is: how one change goes to text in
    /// several formats without making them all one.
    pub fn carry(&mut self, before: &RunFormat, after: &RunFormat) {
        if before.font != after.font {
            self.font = after.font.clone();
        }
        if before.size != after.size {
            self.size = after.size;
        }
        if before.bold != after.bold {
            self.bold = after.bold;
        }
        if before.italic != after.italic {
            self.italic = after.italic;
        }
        if before.underline != after.underline {
            self.underline = after.underline;
        }
        if before.colour != after.colour {
            self.colour = after.colour;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HAlign {
    #[default]
    Left,
    Centre,
    Right,
    Justify,
}

impl HAlign {
    pub const ALL: [HAlign; 4] = [HAlign::Left, HAlign::Centre, HAlign::Right, HAlign::Justify];

    pub fn label(self) -> &'static str {
        match self {
            HAlign::Left => "Left",
            HAlign::Centre => "Centre",
            HAlign::Right => "Right",
            HAlign::Justify => "Justify",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

impl VAlign {
    pub const ALL: [VAlign; 3] = [VAlign::Top, VAlign::Middle, VAlign::Bottom];

    pub fn label(self) -> &'static str {
        match self {
            VAlign::Top => "Top",
            VAlign::Middle => "Middle",
            VAlign::Bottom => "Bottom",
        }
    }
}

impl TextBox {
    /// A box holding `text`, one paragraph a line of it, all in `format`.
    pub fn plain(text: &str, format: &RunFormat, align: HAlign) -> TextBox {
        TextBox { paragraphs: paragraphs_of(text, format, align), valign: VAlign::Top, padding: default_padding(), fit: true, callout: None }
    }

    /// Its words, a line to a paragraph.
    pub fn text(&self) -> String {
        let lines: Vec<String> = self.paragraphs.iter().map(|p| p.runs.iter().map(|r| r.text.as_str()).collect()).collect();
        lines.join("\n")
    }

    /// The format text typed at the end takes: the last run's, or the first
    /// there is.
    pub fn format(&self) -> RunFormat {
        let runs = self.paragraphs.iter().flat_map(|p| &p.runs);
        runs.clone().last().or_else(|| runs.clone().next()).map(|r| r.format.clone()).unwrap_or_default()
    }

    /// How the first paragraph is aligned, which the box's setting shows.
    pub fn align(&self) -> HAlign {
        self.paragraphs.first().map_or(HAlign::Left, |p| p.align)
    }

    /// The same words, all in `format` and aligned `align`.
    pub fn restyled(&self, format: &RunFormat, align: HAlign) -> TextBox {
        TextBox { paragraphs: paragraphs_of(&self.text(), format, align), ..self.clone() }
    }

    /// The same words and settings, the text changed to `text`, in the format
    /// it had.
    pub fn retyped(&self, text: &str) -> TextBox {
        TextBox { paragraphs: paragraphs_of(text, &self.format(), self.align()), ..self.clone() }
    }

    /// Every paragraph aligned `align`.
    pub fn aligned(&self, align: HAlign) -> TextBox {
        let mut aligned = self.clone();
        aligned.paragraphs.iter_mut().for_each(|p| p.align = align);
        aligned
    }

    /// How many characters it holds, a paragraph's end counting as one --
    /// as its `text()` has them, and an editor counts them.
    pub fn char_len(&self) -> usize {
        self.paragraphs.iter().map(|p| p.runs.iter().map(|r| r.text.chars().count()).sum::<usize>()).sum::<usize>() + self.paragraphs.len().saturating_sub(1)
    }

    /// The format text typed at character `at` takes: the character's before
    /// it, or at the start of a paragraph, the first in it.
    pub fn format_at(&self, at: usize) -> RunFormat {
        let flat = Flat::of(self);
        let before = at.checked_sub(1).and_then(|b| flat.cells.get(b)).filter(|(ch, _)| *ch != '\n');
        before.or_else(|| flat.cells.get(at)).map_or(flat.end, |(_, f)| f.clone())
    }

    /// The formats of the characters in `range`, paragraph ends included, so
    /// an empty paragraph picked out has its own; for an empty range, the
    /// one typing there would take.
    pub fn formats_in(&self, range: std::ops::Range<usize>) -> Vec<RunFormat> {
        let flat = Flat::of(self);
        let end = range.end.min(flat.cells.len());
        if range.start >= end {
            return vec![self.format_at(range.start)];
        }
        let mut formats: Vec<RunFormat> = Vec::new();
        for (_, f) in &flat.cells[range.start..end] {
            if !formats.contains(f) {
                formats.push(f.clone());
            }
        }
        formats
    }

    /// The characters in `range` restyled by `change`. Reaching the end, it
    /// restyles what's typed after the last character too.
    pub fn restyled_range(&self, range: std::ops::Range<usize>, change: impl Fn(&mut RunFormat)) -> TextBox {
        let mut flat = Flat::of(self);
        let end = range.end.min(flat.cells.len());
        for (_, f) in flat.cells.iter_mut().take(end).skip(range.start) {
            change(f);
        }
        if range.end >= flat.cells.len() && range.start < range.end.max(1) {
            change(&mut flat.end);
        }
        flat.into_box(self)
    }

    /// The characters in `range` replaced by `text`, in `format`: what an
    /// editor does as it's typed into. A line break in `text` starts a new
    /// paragraph, aligned as the one it broke.
    pub fn replaced(&self, range: std::ops::Range<usize>, text: &str, format: &RunFormat) -> TextBox {
        let mut flat = Flat::of(self);
        let end = range.end.min(flat.cells.len());
        let start = range.start.min(end);
        let paragraph = flat.cells[..start].iter().filter(|(ch, _)| *ch == '\n').count();
        let removed = flat.cells[start..end].iter().filter(|(ch, _)| *ch == '\n').count();
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let added = text.matches('\n').count();
        // Typing into an empty box from the start takes its format for the
        // end as well, which is what's typed next.
        if flat.cells.is_empty() {
            flat.end = format.clone();
        }
        flat.cells.splice(start..end, text.chars().map(|ch| (ch, format.clone())));
        let align = flat.aligns[paragraph];
        flat.aligns.splice(paragraph + 1..paragraph + 1 + removed, std::iter::repeat_n(align, added));
        flat.into_box(self)
    }

    /// `edited` as the edit from this box's words to it: what's the same at
    /// either end kept as it was, and what's new in between typed in
    /// `format`, or in the format where it's typed.
    pub fn edited(&self, edited: &str, format: Option<&RunFormat>) -> TextBox {
        let old: Vec<char> = self.text().chars().collect();
        let new: Vec<char> = edited.chars().collect();
        let same_start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let room = old.len().min(new.len()) - same_start;
        let same_end = old.iter().rev().zip(new.iter().rev()).take(room).take_while(|(a, b)| a == b).count();
        let typed: String = new[same_start..new.len() - same_end].iter().collect();
        let format = format.cloned().unwrap_or_else(|| self.format_at(same_start));
        self.replaced(same_start..old.len() - same_end, &typed, &format)
    }
}

/// A box's words a character at a time, each with its format, a paragraph's
/// end as a line break in the format of the paragraph's last run -- which an
/// empty one keeps as its own -- and the last paragraph's end apart.
struct Flat {
    cells: Vec<(char, RunFormat)>,
    end: RunFormat,
    aligns: Vec<HAlign>,
}

impl Flat {
    fn of(text: &TextBox) -> Flat {
        let mut cells = Vec::new();
        let mut end = RunFormat::default();
        let last = text.paragraphs.len().saturating_sub(1);
        for (i, paragraph) in text.paragraphs.iter().enumerate() {
            for run in &paragraph.runs {
                cells.extend(run.text.chars().map(|ch| (ch, run.format.clone())));
            }
            let own = paragraph.runs.last().map_or_else(RunFormat::default, |r| r.format.clone());
            if i < last {
                cells.push(('\n', own));
            } else {
                end = own;
            }
        }
        let mut aligns: Vec<HAlign> = text.paragraphs.iter().map(|p| p.align).collect();
        if aligns.is_empty() {
            aligns.push(HAlign::Left);
        }
        Flat { cells, end, aligns }
    }

    /// Back into paragraphs of runs, each run as long as its format lasts.
    fn into_box(self, like: &TextBox) -> TextBox {
        let mut paragraphs = Vec::with_capacity(self.aligns.len());
        let mut runs: Vec<Run> = Vec::new();
        let mut finish = |runs: &mut Vec<Run>, own: &RunFormat| {
            if runs.is_empty() {
                runs.push(Run { text: String::new(), format: own.clone() });
            }
            let align = self.aligns.get(paragraphs.len()).copied().unwrap_or_default();
            paragraphs.push(Paragraph { runs: std::mem::take(runs), align });
        };
        for (ch, format) in &self.cells {
            if *ch == '\n' {
                finish(&mut runs, format);
                continue;
            }
            match runs.last_mut() {
                Some(run) if run.format == *format => run.text.push(*ch),
                _ => runs.push(Run { text: ch.to_string(), format: format.clone() }),
            }
        }
        finish(&mut runs, &self.end);
        TextBox { paragraphs, ..like.clone() }
    }
}

/// A box placed on a page by its corners -- its bottom left first, on round
/// anticlockwise -- as a text box and a clip are: where it is, which way its
/// right and up run in user space, and how big it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: Pt,
    /// Unit vectors along the box's bottom and up its left side.
    pub across: Pt,
    pub up: Pt,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// The frame `corners` make; `None` unless they're four with a size.
    pub fn of(corners: &[Pt]) -> Option<Frame> {
        let [origin, right, _, top] = corners else { return None };
        let (along, rise) = (*right - *origin, *top - *origin);
        let (width, height) = (along.len(), rise.len());
        (width > 0.0 && height > 0.0).then(|| Frame { origin: *origin, across: along * (1.0 / width), up: rise * (1.0 / height), width, height })
    }

    /// A point `x` along and `y` up the box, in user space.
    pub fn to_user(&self, x: f64, y: f64) -> Pt {
        self.origin + self.across * x + self.up * y
    }

    /// A point in user space as how far along and up the box it is.
    pub fn to_box(&self, p: Pt) -> (f64, f64) {
        let d = p - self.origin;
        (d.dot(self.across), d.dot(self.up))
    }

    /// The matrix from the box's space to user space, as PDF writes one.
    pub fn matrix(&self) -> [f64; 6] {
        [self.across.x, self.across.y, self.up.x, self.up.y, self.origin.x, self.origin.y]
    }

    /// The corners of the same box `width` by `height`, its bottom left
    /// where it is.
    pub fn corners(&self, width: f64, height: f64) -> Vec<Pt> {
        vec![self.to_user(0.0, 0.0), self.to_user(width, 0.0), self.to_user(width, height), self.to_user(0.0, height)]
    }
}

/// A box's `corners` with corner `corner` taken to `to` and the one across
/// from it kept where it is: any shape, but the same way up, and never less
/// than `least` points along either side -- however far the pointer strays.
pub fn box_resized(corners: &[Pt], corner: usize, to: Pt, least: f64) -> Option<Vec<Pt>> {
    let frame = Frame::of(corners)?;
    if corner > 3 {
        return None;
    }
    // Each corner as a place on the box, 0 or 1 along and up it.
    let unit = |k: usize| match k {
        0 => (0.0, 0.0),
        1 => (1.0, 0.0),
        2 => (1.0, 1.0),
        _ => (0.0, 1.0),
    };
    let opposite = (corner + 2) % 4;
    let fixed = corners[opposite];
    let (dx, dy) = { let d = to - fixed; (d.dot(frame.across), d.dot(frame.up)) };
    let ((cx, cy), (ox, oy)) = (unit(corner), unit(opposite));
    // Along each side, only as far as the right way from the fixed corner.
    let width = (dx * (cx - ox)).max(least);
    let height = (dy * (cy - oy)).max(least);
    Some(
        (0..4)
            .map(|k| {
                let (x, y) = unit(k);
                fixed + frame.across * (width * (x - ox)) + frame.up * (height * (y - oy))
            })
            .collect(),
    )
}

/// Where a box's arrow leaves it for `tip`, in the box's space: the middle
/// of whichever side is nearest the tip.
pub fn callout_start(width: f64, height: f64, (x, y): (f64, f64)) -> (f64, f64) {
    let sides = [(0.0, height / 2.0), (width, height / 2.0), (width / 2.0, 0.0), (width / 2.0, height)];
    sides.into_iter().min_by(|a, b| ((a.0 - x).hypot(a.1 - y)).total_cmp(&(b.0 - x).hypot(b.1 - y))).unwrap_or((0.0, 0.0))
}

fn paragraphs_of(text: &str, format: &RunFormat, align: HAlign) -> Vec<Paragraph> {
    text.split('\n')
        .map(|line| Paragraph { runs: vec![Run { text: line.trim_end_matches('\r').to_owned(), format: format.clone() }], align })
        .collect()
}

fn default_padding() -> f64 {
    4.0
}

fn yes() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_holds_a_paragraph_a_line_and_gives_its_words_back() {
        let format = RunFormat { size: 18.0, ..RunFormat::default() };
        let text = TextBox::plain("Existing kerb\nto be removed", &format, HAlign::Centre);
        assert_eq!(text.paragraphs.len(), 2);
        assert_eq!(text.text(), "Existing kerb\nto be removed");
        assert_eq!((text.format().size, text.align()), (18.0, HAlign::Centre));
        let bold = RunFormat { bold: true, ..format };
        let restyled = text.restyled(&bold, HAlign::Right);
        assert!(restyled.paragraphs.iter().flat_map(|p| &p.runs).all(|r| r.format.bold));
        assert_eq!(restyled.retyped("New kerb").text(), "New kerb");
        assert!(restyled.retyped("New kerb").format().bold, "retyped in the format it had");
    }

    #[test]
    fn a_box_resizes_by_a_corner_keeping_the_one_across_it_and_its_way_up() {
        let square = vec![Pt::new(0.0, 0.0), Pt::new(100.0, 0.0), Pt::new(100.0, 50.0), Pt::new(0.0, 50.0)];
        let wider = box_resized(&square, 2, Pt::new(200.0, 80.0), 5.0).unwrap();
        assert_eq!(wider, vec![Pt::new(0.0, 0.0), Pt::new(200.0, 0.0), Pt::new(200.0, 80.0), Pt::new(0.0, 80.0)], "any shape");
        let past = box_resized(&square, 2, Pt::new(-50.0, -50.0), 5.0).unwrap();
        assert_eq!(past[2], Pt::new(5.0, 5.0), "never turned inside out, never below the least");
        let by_bottom_left = box_resized(&square, 0, Pt::new(-20.0, -10.0), 5.0).unwrap();
        assert_eq!((by_bottom_left[0], by_bottom_left[2]), (Pt::new(-20.0, -10.0), Pt::new(100.0, 50.0)));
        let frame = Frame::of(&square).unwrap();
        assert_eq!((frame.width, frame.height, frame.to_box(Pt::new(30.0, 20.0))), (100.0, 50.0, (30.0, 20.0)));
        assert_eq!(callout_start(100.0, 50.0, (-40.0, 25.0)), (0.0, 25.0), "an arrow to the left leaves by the left side");
    }

    fn runs(text: &TextBox) -> Vec<Vec<(String, bool)>> {
        text.paragraphs.iter().map(|p| p.runs.iter().map(|r| (r.text.clone(), r.format.bold)).collect()).collect()
    }

    #[test]
    fn part_of_a_box_is_restyled_and_typing_takes_the_format_before_it() {
        let text = TextBox::plain("Existing kerb\nto go", &RunFormat::default(), HAlign::Centre);
        assert_eq!(text.char_len(), 19);
        let bold = text.restyled_range(9..13, |f| f.bold = true);
        assert_eq!(runs(&bold), vec![vec![("Existing ".into(), false), ("kerb".into(), true)], vec![("to go".into(), false)]]);
        assert_eq!(bold.formats_in(5..11).len(), 2, "mixed");
        assert!(bold.format_at(13).bold, "typed at the end of the bold word, bold");
        assert!(!bold.format_at(14).bold, "at the start of the next paragraph, as that one starts");

        // Typed in, deleted, and broken into paragraphs, the formats stay.
        let typed = bold.edited("Existing kerbs\nto go", None);
        assert_eq!(runs(&typed)[0], vec![("Existing ".into(), false), ("kerbs".into(), true)]);
        let red = RunFormat { colour: [1.0, 0.0, 0.0], ..RunFormat::default() };
        let broken = typed.edited("Existing\nkerbs\nto go", Some(&red));
        assert_eq!(broken.paragraphs.len(), 3);
        assert!(broken.paragraphs.iter().all(|p| p.align == HAlign::Centre), "a new paragraph aligned as the one it broke");
        assert_eq!(broken.text(), "Existing\nkerbs\nto go");
        let joined = broken.edited("Existingkerbs\nto go", None);
        assert_eq!(runs(&joined)[0], vec![("Existing".into(), false), ("kerbs".into(), true)]);

        // An empty paragraph keeps its own format, which typing there takes.
        let gap = text.edited("Existing kerb\n\nto go", None).restyled_range(14..15, |f| f.size = 30.0);
        assert_eq!(gap.paragraphs[1].runs[0].format.size, 30.0);
        assert_eq!(gap.edited("Existing kerb\nA\nto go", None).paragraphs[1].runs[0].format.size, 30.0);

        // Into an empty box, in the format typing started with.
        let empty = TextBox::plain("", &RunFormat::default(), HAlign::Left);
        let started = empty.edited("Hi", Some(&red));
        assert_eq!(started.format_at(2).colour, [1.0, 0.0, 0.0]);
        assert_eq!(started.format(), red);
    }

    #[test]
    fn a_box_reads_back_as_it_was_written() {
        let mut text = TextBox::plain("A\nB", &RunFormat::default(), HAlign::Left);
        text.callout = Some(Pt::new(10.0, 20.0));
        let back: TextBox = serde_json::from_str(&serde_json::to_string(&text).unwrap()).unwrap();
        assert_eq!(back, text);
    }
}
