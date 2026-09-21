//! The naive redaction: "remove the glyphs from the content stream".
//!
//! This is the STRAWMAN, and it is written as the most plausible first attempt rather than as a
//! bad one, because a strawman nobody would build proves nothing. It tokenises the page's
//! content, finds every text object (`BT` ... `ET`), works out where the text object starts in
//! page space, and deletes the whole object when that origin falls inside the region.
//!
//! What it deliberately does NOT do, because doing it would stop it being the naive version:
//!
//! - follow `/XObject` `Do` into a Form XObject (channel 7);
//! - follow a Type 3 font's `/CharProcs` (channel 8);
//! - look at annotations or their appearance streams (channels 11, 12);
//! - look at anything off the page dictionary at all (channels 10, 15, 16);
//! - understand that `/ActualText` on the enclosing `BDC` states what the glyphs said
//!   (channel 9);
//! - notice pixels (channels 14, 20) or paths (channel 18).
//!
//! Each of those omissions is a row in finding 1, and each is MEASURED by running this rather
//! than argued from this list.
//!
//! It also inherits one thing a real redactor could not: it deletes a whole text object, so
//! text inside the region and text outside it in the same `BT`...`ET` go together. The keep
//! line is in its own text object in every fixture, so that does not silently rescue the
//! measurement -- but it is a limit of the strawman and is stated.

/// A token, with the byte span it occupies, which is the part `pdfsyntax::lexer` does not carry.
#[derive(Debug, Clone)]
enum Tok {
    Num(f64),
    Op(String),
    Other,
}

struct Lexer<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Lexer<'a> {
    fn new(b: &'a [u8]) -> Self {
        Lexer { b, at: 0 }
    }

    fn white(c: u8) -> bool {
        matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
    }

    fn delim(c: u8) -> bool {
        matches!(
            c,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
    }

    fn skip_ws(&mut self) {
        while self.at < self.b.len() {
            let c = self.b[self.at];
            if Self::white(c) {
                self.at += 1;
            } else if c == b'%' {
                while self.at < self.b.len() && self.b[self.at] != b'\n' && self.b[self.at] != b'\r'
                {
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }

    /// The next token and the span `[start, end)` it came from.
    fn next(&mut self) -> Option<(Tok, usize, usize)> {
        self.skip_ws();
        if self.at >= self.b.len() {
            return None;
        }
        let start = self.at;
        let c = self.b[self.at];
        match c {
            b'(' => {
                self.at += 1;
                let mut depth = 1;
                while self.at < self.b.len() && depth > 0 {
                    match self.b[self.at] {
                        b'\\' => self.at += 1,
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    self.at += 1;
                }
                Some((Tok::Other, start, self.at))
            }
            b'<' if self.b.get(self.at + 1) == Some(&b'<') => {
                self.at += 2;
                Some((Tok::Op("<<".into()), start, self.at))
            }
            b'<' => {
                self.at += 1;
                while self.at < self.b.len() && self.b[self.at] != b'>' {
                    self.at += 1;
                }
                self.at = (self.at + 1).min(self.b.len());
                Some((Tok::Other, start, self.at))
            }
            b'>' if self.b.get(self.at + 1) == Some(&b'>') => {
                self.at += 2;
                Some((Tok::Op(">>".into()), start, self.at))
            }
            b'/' | b'[' | b']' | b'{' | b'}' | b'>' | b')' => {
                self.at += 1;
                if c == b'/' {
                    while self.at < self.b.len()
                        && !Self::white(self.b[self.at])
                        && !Self::delim(self.b[self.at])
                    {
                        self.at += 1;
                    }
                }
                Some((Tok::Other, start, self.at))
            }
            _ => {
                while self.at < self.b.len()
                    && !Self::white(self.b[self.at])
                    && !Self::delim(self.b[self.at])
                {
                    self.at += 1;
                }
                let word = &self.b[start..self.at];
                if word.is_empty() {
                    self.at += 1;
                    return Some((Tok::Other, start, self.at));
                }
                let text = String::from_utf8_lossy(word).into_owned();
                match text.parse::<f64>() {
                    Ok(v) => Some((Tok::Num(v), start, self.at)),
                    Err(_) => Some((Tok::Op(text), start, self.at)),
                }
            }
        }
    }
}

/// What the redaction did, reported rather than assumed.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub text_objects: usize,
    pub text_objects_removed: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

/// Delete every text object whose origin falls inside `region`.
///
/// `mutate_to_noop` is control 4: it makes the redactor do nothing while every other part of
/// the harness runs unchanged. A mutation that does not apply is indistinguishable from a
/// defence that holds, so the caller asserts the output differs.
pub fn naive(
    content: &[u8],
    region: (f32, f32, f32, f32),
    mutate_to_noop: bool,
) -> (Vec<u8>, Report) {
    let mut report = Report {
        bytes_before: content.len(),
        ..Default::default()
    };
    if mutate_to_noop {
        report.bytes_after = content.len();
        return (content.to_vec(), report);
    }

    let mut lex = Lexer::new(content);
    let mut operands: Vec<f64> = Vec::new();
    let mut bt: Option<usize> = None;
    let mut origin: Option<(f64, f64)> = None;
    let mut line: (f64, f64) = (0.0, 0.0);
    let mut cuts: Vec<(usize, usize)> = Vec::new();

    while let Some((tok, start, end)) = lex.next() {
        match tok {
            Tok::Num(v) => operands.push(v),
            Tok::Other => operands.clear(),
            Tok::Op(op) => {
                match op.as_str() {
                    "BT" => {
                        bt = Some(start);
                        origin = None;
                        line = (0.0, 0.0);
                        report.text_objects += 1;
                    }
                    "Td" | "TD" => {
                        if operands.len() >= 2 {
                            let (a, b) =
                                (operands[operands.len() - 2], operands[operands.len() - 1]);
                            line = (line.0 + a, line.1 + b);
                            origin = Some(line);
                        }
                    }
                    "Tm" => {
                        if operands.len() >= 6 {
                            let n = operands.len();
                            line = (operands[n - 2], operands[n - 1]);
                            origin = Some(line);
                        }
                    }
                    "ET" => {
                        if let Some(open) = bt.take() {
                            let (x, y) = origin.unwrap_or((0.0, 0.0));
                            let inside = x as f32 >= region.0
                                && x as f32 <= region.2
                                && y as f32 >= region.1
                                && y as f32 <= region.3;
                            if inside {
                                cuts.push((open, end));
                                report.text_objects_removed += 1;
                            }
                        }
                        origin = None;
                    }
                    _ => {}
                }
                operands.clear();
            }
        }
    }

    let mut out = Vec::with_capacity(content.len());
    let mut cursor = 0usize;
    for (from, to) in cuts {
        if from >= cursor {
            out.extend_from_slice(&content[cursor..from]);
            cursor = to;
        }
    }
    out.extend_from_slice(&content[cursor..]);
    report.bytes_after = out.len();
    (out, report)
}
