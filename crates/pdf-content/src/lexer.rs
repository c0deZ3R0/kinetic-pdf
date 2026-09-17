//! Reading a PDF content stream: its operators, each with the operands written
//! before it.

use std::borrow::Cow;
use std::ops::Range;

/// A value written before an operator.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand<'a> {
    /// A number as written; `Operand::number` reads it.
    Number(&'a [u8]),
    Name(&'a [u8]),
    /// A literal string's bytes between its parentheses, escapes as written.
    String(&'a [u8]),
    /// A hexadecimal string's digits between its angle brackets.
    HexString(&'a [u8]),
    Array(Vec<Operand<'a>>),
    Dictionary(Vec<(&'a [u8], Operand<'a>)>),
    Bool(bool),
    Null,
}

impl<'a> Operand<'a> {
    /// The value of a number; `None` for anything else.
    pub fn number(&self) -> Option<f32> {
        match self {
            Operand::Number(text) => std::str::from_utf8(text).ok()?.parse().ok(),
            _ => None,
        }
    }

    /// The bytes of a name; `None` for anything else.
    pub fn name(&self) -> Option<&'a [u8]> {
        match self {
            Operand::Name(name) => Some(name),
            _ => None,
        }
    }

    /// The bytes a string holds, its escapes or hexadecimal digits decoded;
    /// `None` for anything else.
    pub fn bytes(&self) -> Option<Cow<'a, [u8]>> {
        match self {
            Operand::String(raw) if !raw.contains(&b'\\') => Some(Cow::Borrowed(raw)),
            Operand::String(raw) => Some(Cow::Owned(unescape(raw))),
            Operand::HexString(digits) => Some(Cow::Owned(unhex(digits))),
            _ => None,
        }
    }
}

/// A literal string's bytes with its backslash escapes decoded: `\n` and the
/// like, up to three octal digits, a backslash ending a line (which joins
/// it to the next), and anything else standing for itself.
fn unescape(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let byte = raw[i];
        i += 1;
        if byte != b'\\' {
            out.push(byte);
            continue;
        }
        let Some(&escaped) = raw.get(i) else { break };
        i += 1;
        match escaped {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'0'..=b'7' => {
                let mut value = u32::from(escaped - b'0');
                for _ in 0..2 {
                    let Some(&digit @ b'0'..=b'7') = raw.get(i) else { break };
                    value = value * 8 + u32::from(digit - b'0');
                    i += 1;
                }
                out.push(value as u8);
            }
            b'\r' => {
                if raw.get(i) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'\n' => {}
            other => out.push(other),
        }
    }
    out
}

/// A hexadecimal string's bytes: pairs of digits, whitespace ignored, a last
/// digit on its own followed by 0.
fn unhex(digits: &[u8]) -> Vec<u8> {
    let nibbles: Vec<u8> = digits.iter().filter_map(|&d| (d as char).to_digit(16)).map(|n| n as u8).collect();
    nibbles.chunks(2).map(|pair| (pair[0] << 4) | pair.get(1).copied().unwrap_or(0)).collect()
}

/// Calls `visit` for every operator in `content`, in order, with the operands
/// written before it and where the operator itself lies. Anything malformed
/// ends the reading, leaving the rest unvisited.
pub fn each_operation<'a>(content: &'a [u8], mut visit: impl FnMut(&'a [u8], &[Operand<'a>], Range<usize>)) {
    each_operation_while(content, |operator, operands, range| {
        visit(operator, operands, range);
        true
    });
}

/// `each_operation`, stopping as soon as `visit` returns false.
pub fn each_operation_while<'a>(content: &'a [u8], mut visit: impl FnMut(&'a [u8], &[Operand<'a>], Range<usize>) -> bool) {
    let mut operands: Vec<Operand<'a>> = Vec::new();
    // Arrays and dictionaries being read, innermost last, and whether each is
    // a dictionary.
    let mut open: Vec<(Vec<Operand<'a>>, bool)> = Vec::new();
    for token in Tokens::new(content) {
        let value = match token {
            Token::Operator(operator, range) => {
                if open.is_empty() && !visit(operator, &operands, range) {
                    return;
                }
                operands.clear();
                open.clear();
                continue;
            }
            Token::ArrayStart => {
                open.push((Vec::new(), false));
                continue;
            }
            Token::DictionaryStart => {
                open.push((Vec::new(), true));
                continue;
            }
            Token::ArrayEnd | Token::DictionaryEnd => {
                let Some((items, is_dictionary)) = open.pop() else { continue };
                if is_dictionary {
                    Operand::Dictionary(pairs(items))
                } else {
                    Operand::Array(items)
                }
            }
            Token::Operand(value) => value,
        };
        match open.last_mut() {
            Some((items, _)) => items.push(value),
            None => operands.push(value),
        }
    }
}

/// A dictionary's items, read as key, value, key, value.
fn pairs<'a>(items: Vec<Operand<'a>>) -> Vec<(&'a [u8], Operand<'a>)> {
    let mut out = Vec::with_capacity(items.len() / 2);
    let mut items = items.into_iter();
    while let (Some(key), Some(value)) = (items.next(), items.next()) {
        if let Operand::Name(key) = key {
            out.push((key, value));
        }
    }
    out
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
}

fn is_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// One piece of a content stream.
#[derive(Debug, PartialEq)]
enum Token<'a> {
    Operand(Operand<'a>),
    ArrayStart,
    ArrayEnd,
    DictionaryStart,
    DictionaryEnd,
    /// An operator, and where it lies in the stream.
    Operator(&'a [u8], Range<usize>),
}

/// Reads a content stream token by token. Anything it doesn't understand ends
/// it.
struct Tokens<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Tokens<'a> {
    fn new(data: &'a [u8]) -> Self {
        Tokens { data, at: 0 }
    }

    fn skip_while(&mut self, keep: impl Fn(u8) -> bool) {
        while self.at < self.data.len() && keep(self.data[self.at]) {
            self.at += 1;
        }
    }

    /// Skips an inline image's data, which follows `ID` and a single
    /// whitespace byte and runs to `EI` standing on its own.
    fn skip_inline_image(&mut self) {
        let data = self.data;
        let mut i = self.at + 1;
        while i + 2 <= data.len() {
            if data[i] == b'E'
                && data[i + 1] == b'I'
                && is_whitespace(data[i - 1])
                && (i + 2 == data.len() || is_whitespace(data[i + 2]) || is_delimiter(data[i + 2]))
            {
                self.at = i;
                return;
            }
            i += 1;
        }
        self.at = data.len();
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let data = self.data;
        loop {
            self.skip_while(is_whitespace);
            if self.at >= data.len() {
                return None;
            }
            let start = self.at;
            return Some(match data[start] {
                b'%' => {
                    self.skip_while(|b| !matches!(b, b'\r' | b'\n'));
                    continue;
                }
                b'{' | b'}' => {
                    // PostScript calculator braces, never in content streams.
                    self.at += 1;
                    continue;
                }
                b'(' => {
                    // A literal string: balanced parentheses, backslash escapes.
                    let mut depth = 0usize;
                    while self.at < data.len() {
                        match data[self.at] {
                            b'\\' => self.at += 1,
                            b'(' => depth += 1,
                            b')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        self.at += 1;
                    }
                    let end = self.at.min(data.len());
                    self.at = (self.at + 1).min(data.len());
                    Token::Operand(Operand::String(&data[start + 1..end]))
                }
                b'<' if data.get(start + 1) == Some(&b'<') => {
                    self.at += 2;
                    Token::DictionaryStart
                }
                b'>' if data.get(start + 1) == Some(&b'>') => {
                    self.at += 2;
                    Token::DictionaryEnd
                }
                b'<' => {
                    self.skip_while(|b| b != b'>');
                    let end = self.at;
                    self.at = (self.at + 1).min(data.len());
                    Token::Operand(Operand::HexString(&data[start + 1..end]))
                }
                b'[' => {
                    self.at += 1;
                    Token::ArrayStart
                }
                b']' => {
                    self.at += 1;
                    Token::ArrayEnd
                }
                b'/' => {
                    self.at += 1;
                    self.skip_while(|b| !is_whitespace(b) && !is_delimiter(b));
                    Token::Operand(Operand::Name(&data[start + 1..self.at]))
                }
                b')' | b'>' => {
                    // Unbalanced; stop reading.
                    self.at = data.len();
                    return None;
                }
                _ => {
                    self.skip_while(|b| !is_whitespace(b) && !is_delimiter(b));
                    let word = &data[start..self.at];
                    match word {
                        b"true" => Token::Operand(Operand::Bool(true)),
                        b"false" => Token::Operand(Operand::Bool(false)),
                        b"null" => Token::Operand(Operand::Null),
                        _ if word.iter().all(|&b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.')) => {
                            Token::Operand(Operand::Number(word))
                        }
                        _ => {
                            if word == b"ID" {
                                self.skip_inline_image();
                            }
                            Token::Operator(word, start..self.at)
                        }
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every operation in `content`, as its operator's name and operands.
    fn operations(content: &str) -> Vec<(String, Vec<Operand<'_>>)> {
        let mut out = Vec::new();
        each_operation(content.as_bytes(), |operator, operands, _| {
            out.push((String::from_utf8_lossy(operator).into_owned(), operands.to_vec()));
        });
        out
    }

    #[test]
    fn operands_of_every_kind_are_read() {
        let found = operations(r"1 -2.5 .5 /Name (a (b) \) c) <4142> [1 [2] /N] << /K /V /N 3 >> true null op");
        let [(operator, operands)] = &found[..] else { panic!("one operation: {found:?}") };
        assert_eq!(operator, "op");
        assert_eq!(
            operands,
            &[
                Operand::Number(b"1"),
                Operand::Number(b"-2.5"),
                Operand::Number(b".5"),
                Operand::Name(b"Name"),
                Operand::String(br"a (b) \) c"),
                Operand::HexString(b"4142"),
                Operand::Array(vec![Operand::Number(b"1"), Operand::Array(vec![Operand::Number(b"2")]), Operand::Name(b"N")]),
                Operand::Dictionary(vec![(&b"K"[..], Operand::Name(b"V")), (&b"N"[..], Operand::Number(b"3"))]),
                Operand::Bool(true),
                Operand::Null,
            ]
        );
        assert_eq!((operands[1].number(), operands[2].number(), operands[3].name()), (Some(-2.5), Some(0.5), Some(&b"Name"[..])));
    }

    #[test]
    fn strings_decode_their_escapes_and_hex_digits() {
        let found = operations("(plain) (a\\(b\\)\\\\\\101\\n\\7\\1234\\\nc) <48 65 6C6C6F3> Tj");
        let bytes: Vec<Vec<u8>> = found[0].1.iter().map(|o| o.bytes().unwrap().into_owned()).collect();
        assert_eq!(bytes, [b"plain".to_vec(), b"a(b)\\A\n\x07S4c".to_vec(), b"Hello0".to_vec()]);
        assert!(matches!(found[0].1[0].bytes(), Some(Cow::Borrowed(_))), "nothing to decode, nothing copied");
        assert_eq!(Operand::Number(b"1").bytes(), None);
    }

    #[test]
    fn comments_are_skipped_but_a_percent_sign_in_a_string_is_kept() {
        let found = operations("% a comment with an S operator\n(%not a comment) Tj 1 0 0 RG");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], ("Tj".to_owned(), vec![Operand::String(b"%not a comment")]));
        assert_eq!(found[1].0, "RG");
    }

    #[test]
    fn inline_image_data_is_skipped_whole() {
        let names: Vec<String> = operations("BI /W 2 ID aEIb EI 0 g").into_iter().map(|(operator, _)| operator).collect();
        assert_eq!(names, ["BI", "ID", "EI", "g"], "the EI inside the data doesn't end it");
    }

    #[test]
    fn each_operator_says_where_it_lies() {
        let content = b"q 1 0 0 1 5 5 cm Q";
        let mut ranges = Vec::new();
        each_operation(content, |_, _, range| ranges.push(range));
        assert_eq!(ranges.iter().map(|r| &content[r.clone()]).collect::<Vec<_>>(), [&b"q"[..], b"cm", b"Q"]);
    }

    #[test]
    fn reading_stops_at_an_unbalanced_delimiter() {
        assert_eq!(operations("1 w ) 2 w").len(), 1);
    }
}
