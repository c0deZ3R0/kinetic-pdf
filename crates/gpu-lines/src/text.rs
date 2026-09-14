//! Text: the state that places it, and where each glyph goes.

use crate::geometry::Matrix;

/// The text state, part of the graphics state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextState {
    pub char_spacing: f32,
    pub word_spacing: f32,
    /// Horizontal scaling: 1 for none.
    pub scale: f32,
    pub leading: f32,
    /// The font set, by its key in the interpreter's fonts, and its size.
    pub font: Option<usize>,
    pub size: f32,
    pub rise: f32,
    /// The rendering mode, 0 to 7: filled, stroked, both or neither, and from
    /// 4 up the same again, adding the glyphs to the clip.
    pub mode: u8,
}

impl Default for TextState {
    fn default() -> Self {
        TextState { char_spacing: 0.0, word_spacing: 0.0, scale: 1.0, leading: 0.0, font: None, size: 0.0, rise: 0.0, mode: 0 }
    }
}

impl TextState {
    pub fn fills(&self) -> bool {
        matches!(self.mode, 0 | 2 | 4 | 6)
    }

    pub fn strokes(&self) -> bool {
        matches!(self.mode, 1 | 2 | 5 | 6)
    }

    pub fn clips(&self) -> bool {
        self.mode >= 4
    }

    /// The matrix taking a glyph's em square to page space, shown where
    /// `text` says within `ctm`.
    pub fn glyph_matrix(&self, text: Matrix, ctm: Matrix) -> Matrix {
        Matrix([self.size * self.scale, 0.0, 0.0, self.size, 0.0, self.rise]).then(text).then(ctm)
    }

    /// How far along its line the text moves past a glyph `width` thousandths
    /// of an em wide, a space if `is_space`.
    pub fn advance(&self, width: f32, is_space: bool) -> f32 {
        let word_spacing = if is_space { self.word_spacing } else { 0.0 };
        (width / 1000.0 * self.size + self.char_spacing + word_spacing) * self.scale
    }

    /// How far along a number in a `TJ` array moves it: that many thousandths
    /// of an em back.
    pub fn adjust(&self, adjustment: f32) -> f32 {
        -adjustment / 1000.0 * self.size * self.scale
    }
}

/// Where a text object has got to: its text matrix, and the matrix at the
/// start of the line it's on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextObject {
    pub matrix: Matrix,
    pub line: Matrix,
}

impl TextObject {
    pub const START: TextObject = TextObject { matrix: Matrix::IDENTITY, line: Matrix::IDENTITY };

    /// Starts a line offset by `x`, `y` from the start of this one.
    pub fn next_line(&mut self, x: f32, y: f32) {
        self.line = Matrix::translate(x, y).then(self.line);
        self.matrix = self.line;
    }

    /// Starts a line at `matrix`.
    pub fn set(&mut self, matrix: Matrix) {
        (self.matrix, self.line) = (matrix, matrix);
    }

    /// Moves `by` along the line.
    pub fn along(&mut self, by: f32) {
        self.matrix = Matrix::translate(by, 0.0).then(self.matrix);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_are_placed_by_size_scaling_rise_and_the_text_matrix() {
        let text = TextState { size: 10.0, scale: 0.5, rise: 2.0, ..TextState::default() };
        let placed = text.glyph_matrix(Matrix::translate(3.0, 4.0), Matrix::scale(2.0, 2.0));
        assert_eq!(placed.apply([1.0, 1.0]), [(5.0 + 3.0) * 2.0, (10.0 + 2.0 + 4.0) * 2.0]);
    }

    #[test]
    fn advances_add_spacing_then_scale() {
        let text = TextState { size: 10.0, char_spacing: 1.0, word_spacing: 2.0, scale: 0.5, ..TextState::default() };
        assert_eq!(text.advance(500.0, false), 3.0);
        assert_eq!(text.advance(500.0, true), 4.0, "word spacing only after a space");
        assert_eq!(text.adjust(-500.0), 2.5);
    }

    #[test]
    fn a_new_line_starts_from_the_start_of_the_last() {
        let mut text = TextObject::START;
        text.next_line(1.0, 5.0);
        text.along(7.0);
        text.next_line(1.0, -2.0);
        assert_eq!(text.matrix.apply([0.0, 0.0]), [2.0, 3.0]);
    }
}
