//! Every `N G R` qpdf parses, lexed from where qpdf starts parsing (#227).
//!
//! # Why redaction needs this
//!
//! qpdf resolves a reference to an object the cross-reference does not have **at that
//! generation** to null, drops the dictionary key that held it, and warns about nothing. PDFium
//! finds the object by number. So `/Rotate 6 1 R` over a file that holds only `6 0` is no
//! rotation to burrow and a turned page to the viewer, and nothing reachable through qpdf's object
//! graph can tell: the key is gone by the time anything asks. ADR 0029's #227 amendment has the
//! measurements.
//!
//! The owner's rule is to refuse **any** reference qpdf resolves to null. Applying it needs the
//! references as written, which only the bytes still have. This module finds them; the policy in
//! `crate::redact::references` asks qpdf about each one.
//!
//! # Lexed from where qpdf starts, which is what makes it in step with qpdf
//!
//! A scan of the whole file from its first byte is the obvious design and the wrong one: it is in
//! step with qpdf only until the first byte qpdf never tokenises. A `(` in a stream's data, or in
//! junk between two objects, opens a string to a linear scan and to nothing else, and every
//! reference after it is inside that string until a `)` closes it. qpdf skips stream data by its
//! `/Length` and never reads between objects at all, so the two part company exactly where a file
//! puts something to hide.
//!
//! qpdf does not read a file linearly. It parses one value at each place it starts:
//!
//! - after the `obj` keyword of each object it resolves, at the offset the cross-reference gives;
//! - after each `trailer` keyword it reads;
//! - at `/First` plus each member's offset inside a decoded object stream.
//!
//! So this lexes **one value from every such place**, with qpdf's tokenising rules, and from
//! nowhere else. Every `obj` and `trailer` keyword in the file starts a scan -- a superset of the
//! ones qpdf uses, because which offsets the cross-reference names is a question for the
//! cross-reference. A scan from a place qpdf never starts is **not free**: it finds more
//! references to ask about, and it can refuse -- an unreadable header or a reference not written
//! plainly inside an `obj` that is really text, or a reference that resolves to nothing in this
//! document because it belongs to an uncompressed PDF embedded in this one. That is over-refusal,
//! the safe direction, and it is a cost rather than nothing.
//! Each scan begins where qpdf's does, with the same tokeniser, so the two are in step by
//! construction rather than by luck, and stream data is never lexed: a value ends before
//! `stream`.
//!
//! **Where this tokeniser and qpdf's may still differ** is on input qpdf itself complains about --
//! an unterminated string, a stray `)`, an unknown keyword inside a value. qpdf warns on each, and
//! redaction refuses any document qpdf warned about by the write (`[engine-repaired-input]`). So
//! this module needs to agree with qpdf where qpdf is silent. **The first version did not**: it
//! left vertical tab out of white space, which qpdf's `util::is_space` includes, and an object
//! stream headed `7 0\vobj` was a header it never read while qpdf read it silently (security review
//! of #227, round 1). That is fixed, and named here because "written to agree" was the sentence
//! the reviewer disproved.
//!
//! # A reference this cannot read plainly is reported, not guessed at
//!
//! qpdf reads `+6 0 R`, `6 +0 R`, `06 00 R` and `6 %comment` (newline) `0 R` as ordinary
//! references (measured with the CLI). The owner's rule refuses any reference whose numbers are
//! not plain digits, so each of those is [`Irregular::NotPlainDigits`] or
//! [`Irregular::CommentInside`] rather than a number this module decided the reader meant.
//!
//! # Bounded, and not a parser
//!
//! No recursion: nesting is a counter. Every loop advances through the input. The work across all
//! scans is capped at [`WORK_FACTOR`] times the input plus [`WORK_SLACK`], because scans may
//! overlap -- an `obj` inside a string starts a scan through bytes another scan also reads -- and
//! an input built from overlapping starts is refused rather than lexed quadratically. The distinct
//! references are capped at [`MAX_DISTINCT_REFERENCES`]: each costs the caller one engine lookup.
//! Exceeding either is [`Irregular::BeyondCaps`], a refusal, never a shorter answer -- a reference
//! not reported is a reference nobody asks qpdf about.

use std::collections::{BTreeMap, BTreeSet};

use burrow_types::Result;

/// The most distinct references one document may have. Each is one engine lookup.
pub const MAX_DISTINCT_REFERENCES: usize = 1 << 20;

/// The most stream objects whose headers this reports, each a candidate object stream.
pub const MAX_STREAM_OBJECTS: usize = 1 << 20;

/// The most object numbers whose headers this records, and the most members one object stream
/// may list.
pub const MAX_DECLARATIONS: usize = 1 << 20;

/// How many times its own length the lexing of one input may cost, across every scan.
pub const WORK_FACTOR: usize = 4;

/// And a fixed allowance on top, so a small input built from overlapping scans has room to be
/// read rather than refused at its first overlap.
pub const WORK_SLACK: usize = 1 << 20;

/// How many bytes are lexed between two calls of the caller's checkpoint.
const CHECKPOINT_BYTES: usize = 1 << 16;

/// The furthest a header is read backwards over one run of white space or one token.
///
/// A header further spread than this reads as unreadable, which fails closed -- a stream object's
/// refuses, and any other withdraws every declaration -- so that no walk backwards is longer than
/// this between two reads of the deadline. A run of digits cut at this length is past `i64` and
/// cannot be misread as a shorter number.
const MAX_HEADER_WALK: usize = 1 << 12;

/// Why the references could not be read the way qpdf reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Irregular {
    /// A reference whose object number or generation is not plain digits: a sign, a leading zero,
    /// or a value past `i32`. qpdf reads the first two as numbers; nothing here decides which.
    NotPlainDigits,
    /// A comment between a reference's numbers, or between its generation and `R`. qpdf reads
    /// through it.
    CommentInside,
    /// A stream object whose header does not read as two numbers before `obj`, so whether it is
    /// an object stream -- and what its members refer to -- cannot be asked.
    StreamHeaderUnreadable,
    /// An object stream whose header pairs, `/N` or `/First` do not read as qpdf reads them.
    ObjectStreamHeaderUnreadable,
    /// More references, more stream objects or more lexing than the caps allow.
    BeyondCaps,
}

/// What a scan found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Found {
    /// Every reference read plainly, as `(number, generation)` exactly as written.
    pub references: BTreeSet<(i32, i32)>,
    /// The object number of every stream object, from its header: the candidates for object
    /// streams. Empty from [`in_object_stream`], because a stream cannot be a member of one.
    pub stream_objects: BTreeSet<i32>,
    /// Every object header in the file, by object number: what was declared at each generation.
    ///
    /// The evidence that a null qpdf hands back was written by the file rather than made by qpdf.
    /// qpdf's identity is not that evidence: it caches a pair it parses in a trailer before it
    /// knows whether the cross-reference has it, and later fills that entry with a null carrying
    /// the pair (#227, both round-1 reviews). Empty from [`in_object_stream`].
    pub declarations: BTreeMap<i32, Declared>,
    /// Whether some `obj` keyword's header did not read as two numbers. A declaration that
    /// could not be attributed to a number may be any number's, so no null is then taken as
    /// declared.
    pub unreadable_header: bool,
    /// The object numbers an object stream's header lists. Empty from [`in_file`]. A member has
    /// no header in the file, so its declaration cannot be read here.
    pub members: BTreeSet<i32>,
    /// Set when something could not be read as qpdf reads it, and the scan stopped there. The
    /// other fields are then incomplete and must not be used as an answer.
    pub irregular: Option<Irregular>,
}

/// What the headers for one object number declared.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Declared {
    /// The generations at which a header's whole value is `null`.
    pub null_at: BTreeSet<i32>,
    /// Whether some header for this number holds anything else, at any generation.
    pub something_else: bool,
}

impl Found {
    /// Whether the file declares `number` only ever as `null`, and at `generation` among them.
    pub fn declares_only_null(&self, number: i32, generation: i32) -> bool {
        !self.unreadable_header
            && self
                .declarations
                .get(&number)
                .is_some_and(|d| !d.something_else && d.null_at.contains(&generation))
    }
}

/// How a value read by [`Scan::one_value`] ended.
#[derive(Clone, Copy)]
struct ValueEnd {
    /// The next token after the value is `stream`.
    stream: bool,
    /// The value is exactly the keyword `null`, and nothing else.
    only_null: bool,
}

/// Every reference in a whole file, from each `obj` and `trailer` keyword; the object number of
/// every stream object; and what every object header declares ([`Found::declarations`],
/// [`Found::unreadable_header`]).
///
/// **All of it is charged to the budget and the deadline**, the search for the next keyword
/// included, and a header is read backwards at most 4 KiB over bytes the search
/// has charged. Neither was so at first: 512 MiB of `o` ran 2.35 s with no checkpoint, against an
/// amendment that said only one token's lex could pass unread (code review of #227, round 2).
///
/// `checkpoint` is called as the lexing goes, so the caller's deadline is read during it.
///
/// # Errors
///
/// Only whatever `checkpoint` returns. Something unreadable is [`Found::irregular`], not an error.
pub fn in_file(bytes: &[u8], checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<Found> {
    let mut scan = Scan::new(bytes.len(), checkpoint);
    let mut at = 0;
    while at < bytes.len() {
        // `obj` and `trailer` start with these two bytes and nothing else is looked for, so the
        // walk is one comparison per byte -- searched a window at a time, each window charged, so
        // a long stretch with neither byte still reads the deadline.
        let window = bytes
            .get(at..at.saturating_add(CHECKPOINT_BYTES).min(bytes.len()))
            .unwrap_or_default();
        let Some(found) = window.iter().position(|b| *b == b'o' || *b == b't') else {
            if !scan.spend(window.len())? {
                break;
            }
            at = at.saturating_add(window.len());
            continue;
        };
        if !scan.spend(found.saturating_add(1))? {
            break;
        }
        let start = at + found;
        at = start + 1;
        let keyword = if keyword_at(bytes, start, b"obj") {
            b"obj".as_slice()
        } else if keyword_at(bytes, start, b"trailer") {
            b"trailer".as_slice()
        } else {
            continue;
        };
        let end = scan.one_value(bytes, start + keyword.len())?;
        if scan.found.irregular.is_some() {
            break;
        }
        if keyword != b"obj" {
            continue;
        }
        // NOT CHARGED AGAIN: a header is read backwards over bytes the search above has already
        // charged, and at most `MAX_HEADER_WALK` of them. A second charge was tried and no test
        // could tell it was there (round-2 mutation sweep), so it is not claimed.
        let header = header(bytes, start);
        if end.stream {
            let Some((number, _)) = header else {
                scan.found.irregular = Some(Irregular::StreamHeaderUnreadable);
                break;
            };
            scan.found.stream_objects.insert(number);
            if scan.found.stream_objects.len() > MAX_STREAM_OBJECTS {
                scan.found.irregular = Some(Irregular::BeyondCaps);
                break;
            }
        }
        // WHAT THIS HEADER DECLARED. A header that does not read is any number's, as far as
        // anything here can tell, so it withdraws every declaration rather than none.
        let Some((number, generation)) = header else {
            scan.found.unreadable_header = true;
            continue;
        };
        let declared = scan.found.declarations.entry(number).or_default();
        // `!end.stream` is defence in depth, unwitnessed: `null` followed by `stream` is an object
        // qpdf warns about, which refuses after the write.
        if end.only_null && !end.stream {
            declared.null_at.insert(generation);
        } else {
            declared.something_else = true;
        }
        if scan.found.declarations.len() > MAX_DECLARATIONS {
            scan.found.irregular = Some(Irregular::BeyondCaps);
            break;
        }
    }
    Ok(scan.found)
}

/// Every reference in one value, lexed from its start as qpdf lexes it: the text qpdf unparses an
/// object to, for the walk that asks what names a removed annotation (#239).
///
/// # Errors
///
/// Only whatever `checkpoint` returns. Something unreadable is [`Found::irregular`], not an error.
pub fn in_value(text: &[u8], checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<Found> {
    let mut scan = Scan::new(text.len(), checkpoint);
    scan.one_value(text, 0)?;
    Ok(scan.found)
}

/// Every reference in a decoded object stream, from `first` plus each member's offset, as qpdf
/// reads the members.
///
/// `count` and `first` are the stream's `/N` and `/First`. The header is read as qpdf reads it --
/// `count` pairs of integers, comments skipped -- and anything else there is
/// [`Irregular::ObjectStreamHeaderUnreadable`].
///
/// # Errors
///
/// Only whatever `checkpoint` returns.
pub fn in_object_stream(
    decoded: &[u8],
    count: i64,
    first: i64,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<Found> {
    let mut scan = Scan::new(decoded.len(), checkpoint);
    let Some(starts) = scan.member_starts(decoded, count, first)? else {
        scan.found.irregular = Some(Irregular::ObjectStreamHeaderUnreadable);
        return Ok(scan.found);
    };
    if scan.found.irregular.is_some() {
        return Ok(scan.found);
    }
    for start in starts {
        scan.one_value(decoded, start)?;
        if scan.found.irregular.is_some() {
            break;
        }
    }
    Ok(scan.found)
}

/// An integer token's value, sign and leading zeros allowed, or `None` past `i64`.
fn value_of(text: &[u8]) -> Option<i64> {
    let (negative, digits) = match text.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value: i64 = 0;
    for digit in digits {
        value = value
            .checked_mul(10)?
            .checked_add(i64::from(digit.checked_sub(b'0')?))?;
    }
    Some(if negative { -value } else { value })
}

/// A reference number written plainly -- digits only, no leading zero -- as an `i32`.
fn plain(text: &[u8]) -> Option<i32> {
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if text.len() > 1 && text.first() == Some(&b'0') {
        return None;
    }
    i32::try_from(value_of(text)?).ok()
}

/// Whether `keyword` stands at `at` as a token of its own: white space or a delimiter before it,
/// and white space, a delimiter or the end after.
///
/// Before it qpdf has a number and white space -- a comment ends at a line break, which is white
/// space -- so requiring white space or a delimiter finds every `obj` qpdf reads and some it does
/// not, which only starts more scans.
fn keyword_at(bytes: &[u8], at: usize, keyword: &[u8]) -> bool {
    let Some(end) = at.checked_add(keyword.len()) else {
        return false;
    };
    if bytes.get(at..end) != Some(keyword) {
        return false;
    }
    let before = at
        .checked_sub(1)
        .and_then(|i| bytes.get(i))
        .is_some_and(|b| is_whitespace(*b) || is_delimiter(*b));
    let after = bytes
        .get(end)
        .is_none_or(|b| is_whitespace(*b) || is_delimiter(*b));
    before && after
}

/// The number and generation of the header ending in the `obj` at `obj_at`: `N G obj`, read
/// backwards over white space only.
///
/// `None` when the two tokens before `obj` are not integers separated by white space, which
/// includes a header with a comment in it. For a stream object `None` refuses: whether that stream
/// is an object stream cannot then be asked. For any other it withdraws every declaration.
fn header(bytes: &[u8], obj_at: usize) -> Option<(i32, i32)> {
    let before_generation = skip_whitespace_back(bytes, obj_at)?;
    let generation_start = regular_run_back(bytes, before_generation);
    if generation_start == before_generation {
        return None;
    }
    let generation = value_of(bytes.get(generation_start..before_generation)?)?;
    let before_number = skip_whitespace_back(bytes, generation_start)?;
    let number_start = regular_run_back(bytes, before_number);
    if number_start == before_number {
        return None;
    }
    let number = value_of(bytes.get(number_start..before_number)?)?;
    Some((i32::try_from(number).ok()?, i32::try_from(generation).ok()?))
}

/// The position before the white space that ends at `at`, or `None` if there is none or it runs
/// further back than [`MAX_HEADER_WALK`].
fn skip_whitespace_back(bytes: &[u8], at: usize) -> Option<usize> {
    let mut i = at;
    while let Some(previous) = i.checked_sub(1)
        && bytes.get(previous).is_some_and(|b| is_whitespace(*b))
    {
        if at - previous > MAX_HEADER_WALK {
            return None;
        }
        i = previous;
    }
    (i < at).then_some(i)
}

/// Where the run of regular characters ending at `at` begins, read back at most
/// [`MAX_HEADER_WALK`] bytes.
fn regular_run_back(bytes: &[u8], at: usize) -> usize {
    let mut i = at;
    while let Some(previous) = i.checked_sub(1)
        && at - previous <= MAX_HEADER_WALK
        && bytes
            .get(previous)
            .is_some_and(|b| !is_whitespace(*b) && !is_delimiter(*b))
    {
        i = previous;
    }
    i
}

/// The lexing across every value read from one input, and what it found.
struct Scan<'c> {
    found: Found,
    /// Bytes lexed so far, across every scan.
    work: usize,
    /// The most `work` may reach.
    budget: usize,
    /// `work` at the last checkpoint.
    checkpointed: usize,
    checkpoint: &'c mut dyn FnMut() -> Result<()>,
}

/// One token that is not a comment, as far as reading references needs.
#[derive(Clone, Copy)]
enum Significant<'a> {
    Integer(&'a [u8]),
    Other,
}

impl<'c> Scan<'c> {
    fn new(length: usize, checkpoint: &'c mut dyn FnMut() -> Result<()>) -> Self {
        Self {
            found: Found::default(),
            work: 0,
            budget: length
                .saturating_mul(WORK_FACTOR)
                .saturating_add(WORK_SLACK),
            checkpointed: 0,
            checkpoint,
        }
    }

    /// Lex one value from `start`, recording every reference in it, and say whether the next
    /// token after the value is `stream`.
    ///
    /// A value ends when its outermost container closes, or at a scalar outside any container --
    /// an integer only once it cannot begin a reference. It also ends at a keyword that cannot be
    /// inside a value qpdf reads without warning (`obj`, `endobj`, `stream`, `endstream`, `xref`,
    /// `trailer`, `startxref`), and at the end of the input. Lexing past the value qpdf would read
    /// can only find more, so the rule leans long.
    fn one_value(&mut self, bytes: &[u8], start: usize) -> Result<ValueEnd> {
        let mut lexer = Lexer::new(bytes, start);
        // Whether every significant token so far is one `null`: the value of a declared null.
        let mut significant_tokens = 0_usize;
        let mut first_is_null = false;
        let end = |stream: bool, significant_tokens: usize, first_is_null: bool| ValueEnd {
            stream,
            only_null: significant_tokens == 1 && first_is_null,
        };
        let mut depth: usize = 0;
        // The two significant tokens before this one, the newer first, and whether a comment
        // came between each and the one after it.
        let mut newer: Option<(Significant<'_>, bool)> = None;
        let mut older: Option<(Significant<'_>, bool)> = None;
        let mut comment_since_newer = false;
        let mut integers_outside = 0_usize;
        let mut complete = false;
        loop {
            let before = lexer.at;
            let Some(token) = lexer.next() else {
                return Ok(end(false, significant_tokens, first_is_null));
            };
            if !self.spend(lexer.at.saturating_sub(before))? {
                return Ok(end(false, 0, false));
            }
            if complete {
                // ONE MORE SIGNIFICANT TOKEN, to see whether a stream follows the value.
                match token {
                    Token::Comment => continue,
                    Token::Word(word) => {
                        return Ok(end(word == b"stream", significant_tokens, first_is_null));
                    }
                    _ => return Ok(end(false, 0, false)),
                }
            }
            let significant = match token {
                Token::Comment => {
                    comment_since_newer = true;
                    continue;
                }
                Token::Integer(text) => Significant::Integer(text),
                Token::Word(b"R") => {
                    if let (
                        Some((Significant::Integer(generation), comment_before_generation)),
                        Some((Significant::Integer(number), _)),
                    ) = (newer, older)
                    {
                        self.reference(
                            number,
                            generation,
                            comment_before_generation || comment_since_newer,
                        );
                        if self.found.irregular.is_some() {
                            return Ok(end(false, 0, false));
                        }
                    }
                    Significant::Other
                }
                Token::Word(word) if is_stop_word(word) => {
                    return Ok(end(
                        word == b"stream" && depth == 0,
                        significant_tokens,
                        first_is_null,
                    ));
                }
                _ => Significant::Other,
            };
            significant_tokens = significant_tokens.saturating_add(1);
            if significant_tokens == 1 {
                first_is_null = token == Token::Word(b"null");
            }
            older = newer;
            newer = Some((significant, comment_since_newer));
            comment_since_newer = false;
            match token {
                Token::Open => depth = depth.saturating_add(1),
                Token::Close => {
                    depth = depth.saturating_sub(1);
                    complete = depth == 0;
                }
                Token::Integer(_) if depth == 0 => {
                    integers_outside += 1;
                    complete = integers_outside > 2;
                }
                _ if depth == 0 => complete = true,
                _ => {}
            }
        }
    }

    /// Where each member of an object stream begins, as offsets into `decoded`, recording each
    /// member's number in [`Found::members`]; `None` when the header does not read as `count`
    /// pairs of integers or `first` lies outside the data.
    ///
    /// Every pair is kept, including ones qpdf skips: a skipped member is one more place a value
    /// is read from, and qpdf warns about the skips that matter (`QPDF_objects.cc`,
    /// `resolveObjectsInStream`). The one it skips silently -- a number above the
    /// cross-reference's largest -- is still lexed here, which can only find more.
    ///
    /// **Charged to the budget and the deadline, and capped**, as the values are: the security
    /// review of #227's first round measured 1.41 s and 639 MB on a 229 MB header before the first
    /// checkpoint, when this loop was outside both.
    fn member_starts(
        &mut self,
        decoded: &[u8],
        count: i64,
        first: i64,
    ) -> Result<Option<BTreeSet<usize>>> {
        let (Ok(first), Ok(count)) = (usize::try_from(first), usize::try_from(count)) else {
            return Ok(None);
        };
        if first >= decoded.len() {
            return Ok(None);
        }
        // Two tokens a pair and at least one byte a token, so a count past the data cannot be met.
        if count.saturating_mul(2) > decoded.len().saturating_add(1) {
            return Ok(None);
        }
        if count > MAX_DECLARATIONS {
            self.found.irregular = Some(Irregular::BeyondCaps);
            return Ok(Some(BTreeSet::new()));
        }
        let mut lexer = Lexer::new(decoded, 0);
        let mut starts = BTreeSet::new();
        for _ in 0..count {
            let Some(number) = self.integer(&mut lexer)? else {
                return Ok(self.found.irregular.map(|_| BTreeSet::new()));
            };
            let Some(offset) = self.integer(&mut lexer)? else {
                return Ok(self.found.irregular.map(|_| BTreeSet::new()));
            };
            if let Ok(number) = i32::try_from(number) {
                self.found.members.insert(number);
            }
            let Ok(offset) = usize::try_from(offset) else {
                continue;
            };
            if let Some(start) = first.checked_add(offset)
                && start < decoded.len()
            {
                starts.insert(start);
            }
        }
        Ok(Some(starts))
    }

    /// The next token that is not a comment, if it is an integer qpdf would read, as its value,
    /// charged to the budget. `None` for anything else, and when the budget is spent -- with
    /// [`Irregular::BeyondCaps`] recorded, which the caller tells apart.
    fn integer(&mut self, lexer: &mut Lexer<'_>) -> Result<Option<i64>> {
        loop {
            let before = lexer.at;
            let token = lexer.next();
            if !self.spend(lexer.at.saturating_sub(before))? {
                return Ok(None);
            }
            match token {
                Some(Token::Comment) => {}
                Some(Token::Integer(text)) => return Ok(value_of(text)),
                _ => return Ok(None),
            }
        }
    }

    /// Record the reference `number generation R`.
    fn reference(&mut self, number: &[u8], generation: &[u8], comment_inside: bool) {
        if comment_inside {
            self.found.irregular = Some(Irregular::CommentInside);
            return;
        }
        let (Some(number), Some(generation)) = (plain(number), plain(generation)) else {
            self.found.irregular = Some(Irregular::NotPlainDigits);
            return;
        };
        self.found.references.insert((number, generation));
        if self.found.references.len() > MAX_DISTINCT_REFERENCES {
            self.found.irregular = Some(Irregular::BeyondCaps);
        }
    }

    /// Count `bytes` of lexing against the budget, calling the checkpoint as it goes. `false`
    /// when the budget is spent, with [`Irregular::BeyondCaps`] recorded.
    fn spend(&mut self, bytes: usize) -> Result<bool> {
        self.work = self.work.saturating_add(bytes);
        if self.work.saturating_sub(self.checkpointed) >= CHECKPOINT_BYTES {
            self.checkpointed = self.work;
            (self.checkpoint)()?;
        }
        if self.work > self.budget {
            self.found.irregular = Some(Irregular::BeyondCaps);
            return Ok(false);
        }
        Ok(true)
    }
}

/// A keyword that cannot be inside a value qpdf reads without warning.
fn is_stop_word(word: &[u8]) -> bool {
    matches!(
        word,
        b"obj" | b"endobj" | b"stream" | b"endstream" | b"xref" | b"trailer" | b"startxref"
    )
}

/// One token, as qpdf's tokeniser would split it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// `[` or `<<`.
    Open,
    /// `]` or `>>`.
    Close,
    /// A run of regular characters that qpdf reads as an integer: an optional sign and digits.
    Integer(&'a [u8]),
    /// Any other run of regular characters: a keyword, a real, `R`.
    Word(&'a [u8]),
    /// `%` to the end of the line.
    Comment,
    /// A string, a hex string, a name, or a delimiter qpdf would warn about.
    Scalar,
}

/// A cursor over PDF object syntax, splitting tokens where qpdf's `QPDFTokenizer` does.
struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Lexer<'a> {
    const fn new(bytes: &'a [u8], at: usize) -> Self {
        Self { bytes, at }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    /// The next token, or `None` at the end of the input. Never fails: input qpdf would warn
    /// about still splits somewhere, and the warning is what refuses it.
    fn next(&mut self) -> Option<Token<'a>> {
        while self.peek().is_some_and(is_whitespace) {
            self.at += 1;
        }
        let first = self.peek()?;
        self.at += 1;
        Some(match first {
            b'%' => {
                while self.peek().is_some_and(|b| b != b'\r' && b != b'\n') {
                    self.at += 1;
                }
                Token::Comment
            }
            b'(' => {
                self.literal_string();
                Token::Scalar
            }
            b'<' if self.peek() == Some(b'<') => {
                self.at += 1;
                Token::Open
            }
            b'<' => {
                while let Some(b) = self.peek() {
                    self.at += 1;
                    if b == b'>' {
                        break;
                    }
                }
                Token::Scalar
            }
            b'>' if self.peek() == Some(b'>') => {
                self.at += 1;
                Token::Close
            }
            b'[' => Token::Open,
            b']' => Token::Close,
            b'/' => {
                self.regular_run();
                Token::Scalar
            }
            b')' | b'>' | b'{' | b'}' => Token::Scalar,
            _ => {
                let start = self.at - 1;
                self.regular_run();
                let text = self.bytes.get(start..self.at).unwrap_or_default();
                if is_integer(text) {
                    Token::Integer(text)
                } else {
                    Token::Word(text)
                }
            }
        })
    }

    /// Past the rest of a literal string whose `(` was just read: balanced parentheses, and a
    /// backslash takes the byte after it with it, so `\)` closes nothing. To the end of the input
    /// if it never closes.
    fn literal_string(&mut self) {
        let mut depth: usize = 1;
        while let Some(b) = self.peek() {
            self.at += 1;
            match b {
                b'\\' => {
                    if self.peek().is_some() {
                        self.at += 1;
                    }
                }
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    fn regular_run(&mut self) {
        while self
            .peek()
            .is_some_and(|b| !is_whitespace(b) && !is_delimiter(b))
        {
            self.at += 1;
        }
    }
}

/// Whether `text` is what qpdf reads as an integer: an optional sign, then one or more digits.
fn is_integer(text: &[u8]) -> bool {
    let digits = match text.split_first() {
        Some((b'+' | b'-', rest)) => rest,
        _ => text,
    };
    !digits.is_empty() && digits.iter().all(u8::is_ascii_digit)
}

/// White space as qpdf's tokeniser reads it: PDF's six, NUL among them, **and vertical tab**.
///
/// qpdf's `util::is_space` (`qpdf/Util.hh`) includes `\v`, which PDF does not list. The first
/// version of this omitted it, so `7 0\vobj` heading an object stream was a header this lexer
/// never read and every member went unchecked, with qpdf silent (security review of #227, round
/// 1). Only `\v` is added: PDFium's own extra white space would end a scan at bytes qpdf reads
/// as part of a token.
const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ')
}

/// PDF's delimiters, as qpdf's `QPDFTokenizer::is_delimiter`.
const fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

#[cfg(test)]
mod tests {
    use super::{Found, Irregular, MAX_DISTINCT_REFERENCES, in_file, in_object_stream};
    use burrow_types::{Error, Result};
    use proptest::prelude::*;

    fn file(bytes: &[u8]) -> Found {
        in_file(bytes, &mut || Ok(())).expect("no checkpoint fails")
    }

    fn object_stream(decoded: &[u8], count: i64, first: i64) -> Found {
        in_object_stream(decoded, count, first, &mut || Ok(())).expect("no checkpoint fails")
    }

    fn refs(found: &Found) -> Vec<(i32, i32)> {
        assert_eq!(found.irregular, None, "read plainly: {found:?}");
        found.references.iter().copied().collect()
    }

    #[test]
    fn a_reference_in_an_object_is_read_with_the_generation_written() {
        let found = file(b"1 0 obj << /Rotate 6 1 R /Parent 2 0 R >> endobj");
        assert_eq!(refs(&found), [(2, 0), (6, 1)]);
        assert!(found.stream_objects.is_empty());
    }

    #[test]
    fn references_in_arrays_nested_values_and_a_bare_value_are_read() {
        assert_eq!(
            refs(&file(b"3 0 obj [1 [4 0 R] << /K [5 2 R] >> 7] endobj")),
            [(4, 0), (5, 2)]
        );
        // An object whose whole value is a reference.
        assert_eq!(refs(&file(b"5 0 obj 6 0 R endobj")), [(6, 0)]);
    }

    #[test]
    fn a_trailer_is_read_as_qpdf_reads_it() {
        let found = file(b"xref\n0 1\n0000000000 65535 f \ntrailer << /Root 1 0 R /Info 9 1 R >>\nstartxref\n0\n%%EOF");
        assert_eq!(refs(&found), [(1, 0), (9, 1)]);
    }

    #[test]
    fn a_number_that_is_not_plain_digits_is_irregular_one_shape_at_a_time() {
        // qpdf reads each of these as a reference (measured with the CLI before #227). The rule
        // refuses them rather than choosing a value.
        for shape in [
            b"+6 0 R".as_slice(),
            b"6 +0 R",
            b"06 00 R",
            b"06 0 R",
            b"6 00 R",
            b"-6 0 R",
            b"6 -0 R",
            b"2147483648 0 R",
        ] {
            let mut bytes = b"1 0 obj << /Rotate ".to_vec();
            bytes.extend_from_slice(shape);
            bytes.extend_from_slice(b" >> endobj");
            assert_eq!(
                file(&bytes).irregular,
                Some(Irregular::NotPlainDigits),
                "{}",
                String::from_utf8_lossy(shape)
            );
        }
        // THE NEAR-MISS: plain zero and the largest plain number are plain.
        assert_eq!(
            refs(&file(b"1 0 obj [0 0 R 2147483647 65535 R] endobj")),
            [(0, 0), (2_147_483_647, 65535)]
        );
    }

    #[test]
    fn a_comment_inside_a_reference_is_irregular_wherever_it_sits() {
        for shape in [b"6 %c\n0 R".as_slice(), b"6 0 %c\nR", b"6%c\n0%d\rR"] {
            let mut bytes = b"1 0 obj << /Rotate ".to_vec();
            bytes.extend_from_slice(shape);
            bytes.extend_from_slice(b" >> endobj");
            assert_eq!(
                file(&bytes).irregular,
                Some(Irregular::CommentInside),
                "{}",
                String::from_utf8_lossy(shape)
            );
        }
        // THE NEAR-MISS: a comment beside a reference, not inside it.
        assert_eq!(
            refs(&file(b"1 0 obj << %c\n/Rotate 6 0 R %d\n>> endobj")),
            [(6, 0)]
        );
    }

    #[test]
    fn a_reference_inside_a_string_or_a_name_is_not_one() {
        assert_eq!(
            refs(&file(
                b"1 0 obj << /T (6 1 R) /U <36> /6 1 /V (a\\) 7 1 R) >> endobj"
            )),
            Vec::<(i32, i32)>::new()
        );
    }

    #[test]
    fn stream_data_cannot_open_a_string_over_the_objects_after_it() {
        // THE REASON THIS IS NOT A LINEAR SCAN. A `(` in stream data opens a string to a lexer
        // that reads the file from its first byte and to nothing else; qpdf skips the data by
        // `/Length`. Each object here is read from its own `obj`, so the `(` cannot reach it.
        let found = file(
            b"4 0 obj << /Length 3 >> stream\n(((\nendstream endobj\n\
              5 0 obj << /Rotate 6 1 R >> endobj",
        );
        assert_eq!(refs(&found), [(6, 1)]);
        // And a `%` with the next object on the same line, which a linear scan reads as comment.
        let found =
            file(b"4 0 obj << /Length 2 >> stream\n%x endstream endobj 5 0 obj [6 1 R] endobj");
        assert_eq!(refs(&found), [(6, 1)]);
    }

    #[test]
    fn junk_between_objects_cannot_hide_the_next_one() {
        let found = file(b"1 0 obj << >> endobj ( %\n2 0 obj << /Rotate 6 1 R >> endobj");
        assert_eq!(refs(&found), [(6, 1)]);
    }

    #[test]
    fn a_stream_object_is_reported_by_its_header_number() {
        let found =
            file(b"12 0 obj << /Type /ObjStm /N 1 /First 4 >>\nstream\nxx\nendstream\nendobj");
        assert_eq!(
            found.stream_objects.iter().copied().collect::<Vec<_>>(),
            [12]
        );
        // A non-stream object is not a candidate.
        assert!(file(b"12 0 obj << >> endobj").stream_objects.is_empty());
        // A header number past i32, or with a comment in it, cannot be asked about.
        for header in [
            b"12 %c\n0 obj".as_slice(),
            b"2147483648 0 obj",
            b"x12 0 obj",
        ] {
            let mut bytes = header.to_vec();
            bytes.extend_from_slice(b" << >> stream\nxx\nendstream endobj");
            assert_eq!(
                file(&bytes).irregular,
                Some(Irregular::StreamHeaderUnreadable),
                "{}",
                String::from_utf8_lossy(header)
            );
        }
        // A header with a sign or a leading zero is still a number qpdf reads, and is asked as one.
        let found = file(b"+012 0 obj << >> stream\nxx\nendstream endobj");
        assert_eq!(
            found.stream_objects.iter().copied().collect::<Vec<_>>(),
            [12]
        );
    }

    #[test]
    fn obj_inside_endobj_or_a_longer_word_starts_nothing() {
        assert_eq!(
            refs(&file(b"(endobj 6 1 R) xobj 7 1 R objx 8 1 R")),
            Vec::<(i32, i32)>::new()
        );
    }

    #[test]
    fn object_stream_members_are_read_from_first_plus_each_offset() {
        // Two members: `<< /Rotate 6 1 R >>` at 0 and `[7 0 R]` at 20.
        let body = b"<< /Rotate 6 1 R >> [7 0 R]";
        let header = b"10 0 11 20 ";
        let mut decoded = header.to_vec();
        decoded.extend_from_slice(body);
        let first = i64::try_from(header.len()).expect("small");
        assert_eq!(refs(&object_stream(&decoded, 2, first)), [(6, 1), (7, 0)]);
    }

    #[test]
    fn a_member_whose_value_runs_past_the_next_offset_is_read_to_its_end() {
        // qpdf parses one value from each member's start. If the next pair is one it skips
        // silently -- a number above the cross-reference's largest -- the value runs through
        // the bytes that pair named, and a reference split across them with a comment inside is
        // a reference to qpdf. Read from each start to the value's end, it is seen here too.
        let header = b"10 0 999999 7 ";
        let mut decoded = header.to_vec();
        decoded.extend_from_slice(b"[ 6 %\n 1 R ]");
        let first = i64::try_from(header.len()).expect("small");
        assert_eq!(
            object_stream(&decoded, 2, first).irregular,
            Some(Irregular::CommentInside)
        );
    }

    #[test]
    fn an_object_stream_header_qpdf_would_not_read_is_irregular() {
        for (decoded, count, first) in [
            (b"10 x [6 0 R]".as_slice(), 1, 5),
            (b"10 0.5 [6 0 R]", 1, 7),
            (b"10 0 [6 0 R]", 1, 99),
            (b"10 0 [6 0 R]", 1, -1),
            (b"10 0 [6 0 R]", 50, 5),
            (b"10 0 [6 0 R]", -1, 5),
        ] {
            assert_eq!(
                object_stream(decoded, count, first).irregular,
                Some(Irregular::ObjectStreamHeaderUnreadable),
                "{} /N {count} /First {first}",
                String::from_utf8_lossy(decoded)
            );
        }
        // THE NEAR-MISS: a comment in the header is skipped, as qpdf's tokeniser skips it, and the
        // member it lists is read and recorded.
        let found = object_stream(b"10 %c\n 0 [6 0 R]", 1, 9);
        assert_eq!(refs(&found), [(6, 0)]);
        assert_eq!(found.members.iter().copied().collect::<Vec<_>>(), [10]);
    }

    #[test]
    fn overlapping_scans_are_refused_rather_than_lexed_quadratically() {
        // Every `obj` starts a scan, and an unclosed string runs each to the end of the input.
        let bytes = b" obj (".repeat(200_000);
        assert_eq!(file(&bytes).irregular, Some(Irregular::BeyondCaps));
        // THE NEAR-MISS: the same number of objects, each closed, is read.
        let bytes = b" obj () endobj".repeat(200_000);
        assert_eq!(file(&bytes).irregular, None);
    }

    #[test]
    fn more_distinct_references_than_the_cap_are_refused() {
        let mut bytes = b"1 0 obj [".to_vec();
        for n in 1..=MAX_DISTINCT_REFERENCES + 1 {
            bytes.extend_from_slice(format!("{n} 0 R ").as_bytes());
        }
        bytes.extend_from_slice(b"] endobj");
        assert_eq!(file(&bytes).irregular, Some(Irregular::BeyondCaps));
        // THE NEAR-MISS: exactly the cap is read.
        let at = bytes
            .windows(b"1048577 0 R".len())
            .position(|w| w == b"1048577 0 R")
            .expect("the last reference is there");
        let mut exactly = bytes[..at].to_vec();
        exactly.extend_from_slice(b"] endobj");
        assert_eq!(file(&exactly).references.len(), MAX_DISTINCT_REFERENCES);
    }

    #[test]
    fn the_deadline_is_read_while_it_lexes() {
        let bytes = b"1 0 obj ["
            .iter()
            .chain(b"7 0 R ".repeat(100_000).iter())
            .copied()
            .collect::<Vec<u8>>();
        let mut calls = 0;
        let error = in_file(&bytes, &mut || {
            calls += 1;
            if calls > 3 {
                Err(Error::Internal("planted deadline".to_owned()))
            } else {
                Ok(())
            }
        })
        .expect_err("a checkpoint that fails ends the scan");
        assert!(matches!(error, Error::Internal(ref m) if m == "planted deadline"));
        // And an object stream's scan reads it too.
        let mut called = false;
        let decoded = b"10 0 ["
            .iter()
            .chain(b"7 0 R ".repeat(100_000).iter())
            .copied()
            .collect::<Vec<u8>>();
        let _: Result<Found> = in_object_stream(&decoded, 1, 5, &mut || {
            called = true;
            Ok(())
        });
        assert!(
            called,
            "the object stream's lexing never asked the deadline"
        );
    }

    #[test]
    fn a_number_is_declared_null_only_if_every_header_for_it_holds_null() {
        // THE EVIDENCE A NULL WAS WRITTEN (#227, round 2). qpdf's identity is not it: qpdf caches
        // a pair a trailer names and later fills it with a null carrying that pair.
        let found = file(
            b"6 0 obj null endobj\n6 1 obj 90 endobj\n7 0 obj null %c\nendobj\n\
              8 0 obj << >> stream\nxx\nendstream endobj\n9 0 obj [null] endobj\n",
        );
        assert!(
            found.declares_only_null(7, 0),
            "a header holding only null declares it"
        );
        assert!(
            !found.declares_only_null(7, 1),
            "at that generation, not another"
        );
        assert!(
            !found.declares_only_null(6, 0),
            "not where another header for 6 holds 90"
        );
        assert!(!found.declares_only_null(8, 0), "a stream is not null");
        assert!(
            !found.declares_only_null(9, 0),
            "an array holding null is not null"
        );
        assert!(
            !found.declares_only_null(99, 0),
            "and nothing is declared without a header"
        );
    }

    #[test]
    fn a_header_that_does_not_read_withdraws_every_declaration() {
        // A value under a header nothing can attribute may be any number's -- `7 1 obj 90` written
        // with a comment in its header, say -- so no null is taken as declared.
        let found = file(b"7 %c\n1 obj 90 endobj\n7 0 obj null endobj\n");
        assert!(found.unreadable_header);
        assert!(!found.declares_only_null(7, 0));
        // THE NEAR-MISS: the same file with a plain header declares 7 at 1 as something else, and
        // still nothing withdraws the rest.
        let found = file(b"8 1 obj 90 endobj\n7 0 obj null endobj\n");
        assert!(!found.unreadable_header);
        assert!(found.declares_only_null(7, 0));
    }

    #[test]
    fn vertical_tab_is_white_space_as_qpdf_reads_it() {
        // qpdf's `util::is_space` includes `\v`. Without it, `7 0\vobj` was a header this never
        // read, and an object stream headed that way had its members unchecked, qpdf silent.
        let found =
            file(b"7 0\x0bobj << /Type /ObjStm /N 1 /First 4 >> stream\nxx\nendstream endobj");
        assert_eq!(
            found.stream_objects.iter().copied().collect::<Vec<_>>(),
            [7]
        );
        assert_eq!(refs(&file(b"1 0 obj [6\x0b1\x0bR] endobj")), [(6, 1)]);
        // And NUL and form feed, which the security review showed are load-bearing: a reference
        // split by either is a reference to qpdf, and PDFium turns the page through it.
        assert_eq!(
            refs(&file(b"1 0 obj [6\x001 R 7 1\x0cR] endobj")),
            [(6, 1), (7, 1)]
        );
    }

    #[test]
    fn an_object_stream_header_is_charged_to_the_budget_and_the_deadline() {
        // MEASURED UNBOUNDED BEFORE: 229 MB of header pairs ran 1.41 s with no checkpoint.
        let header = b"1 0 ".repeat(100_000);
        let mut decoded = header.clone();
        decoded.extend_from_slice(b"null");
        let mut called = 0;
        let found = in_object_stream(
            &decoded,
            100_000,
            i64::try_from(header.len()).unwrap(),
            &mut || {
                called += 1;
                Ok(())
            },
        )
        .unwrap();
        assert!(called > 0, "the header's lexing never read the deadline");
        assert_eq!(found.irregular, None);
        assert_eq!(found.members.iter().copied().collect::<Vec<_>>(), [1]);
        // And a count past the cap refuses rather than reading on.
        let found = object_stream(
            &b"1 0 "
                .repeat(1 << 20)
                .iter()
                .chain(b"null x")
                .copied()
                .collect::<Vec<_>>(),
            (1 << 20) + 1,
            8,
        );
        assert_eq!(found.irregular, Some(Irregular::BeyondCaps));
    }

    #[test]
    fn the_keyword_search_and_the_header_walk_read_the_deadline() {
        // MEASURED UNCHARGED BEFORE (code review of #227, round 2): 512 MiB of `o` ran 2.35 s, and
        // `t ` repeated 1.25 s, with no checkpoint -- neither is a value, so no value's lexing
        // charged them. One MiB of each must ask the deadline at least once.
        for input in [
            vec![b'o'; 1 << 20],
            b"t ".repeat(1 << 19),
            vec![b'x'; 1 << 20],
        ] {
            let mut called = 0;
            let _ = in_file(&input, &mut || {
                called += 1;
                Ok(())
            });
            assert!(
                called > 0,
                "1 MiB of {:?} never read the deadline",
                &input[..2]
            );
        }
    }

    #[test]
    fn a_header_spread_past_the_walk_bound_reads_as_unreadable() {
        // `1`, a MiB of spaces, `0 obj null`: qpdf may read that header; a walk back over the
        // whole run would be a long stretch with no deadline read, so it is not taken, and the
        // header withdraws every declaration -- the fail-closed direction.
        let mut bytes = b"1".to_vec();
        bytes.extend(std::iter::repeat_n(b' ', 1 << 20));
        bytes.extend_from_slice(b"0 obj null endobj");
        let found = file(&bytes);
        assert!(found.unreadable_header);
        assert!(!found.declares_only_null(1, 0));
        // THE NEAR-MISS: the same header within the bound reads.
        let found = file(b"1        0 obj null endobj");
        assert!(found.declares_only_null(1, 0));
    }

    #[test]
    fn more_declared_numbers_than_the_cap_are_refused() {
        let mut bytes = Vec::new();
        for n in 1..=super::MAX_DECLARATIONS + 1 {
            bytes.extend_from_slice(format!("{n} 0 obj null endobj\n").as_bytes());
        }
        assert_eq!(file(&bytes).irregular, Some(Irregular::BeyondCaps));
        // THE NEAR-MISS: exactly the cap is read.
        let last = format!("{} 0 obj", super::MAX_DECLARATIONS + 1);
        let at = bytes
            .windows(last.len())
            .position(|w| w == last.as_bytes())
            .expect("the last header is there");
        let found = file(&bytes[..at]);
        assert_eq!(found.irregular, None);
        assert_eq!(found.declarations.len(), super::MAX_DECLARATIONS);
    }

    /// A reference as written, plain or not.
    fn reference_text() -> impl Strategy<Value = (i32, i32)> {
        (0..100_000_i32, 0..70_000_i32)
    }

    /// Bytes a producer might put between objects or in stream data: any of the delimiters that
    /// open a mode in a linear scan.
    fn junk() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(
            prop_oneof![
                Just(b'('),
                Just(b')'),
                Just(b'%'),
                Just(b'<'),
                Just(b'\\'),
                Just(b' '),
                Just(b'x'),
            ],
            0..12,
        )
    }

    proptest! {
        #[test]
        fn every_reference_written_into_an_object_is_found_whatever_lies_between_objects(
            objects in proptest::collection::vec(
                (proptest::collection::vec(reference_text(), 0..6), junk(), any::<bool>()),
                1..8,
            ),
        ) {
            // Each object holds its references in an array, and optionally carries stream data
            // made of junk; junk also lies between objects. Whatever the junk opens, each
            // object is read from its own `obj`.
            let mut bytes = Vec::new();
            let mut expected = std::collections::BTreeSet::new();
            for (number, (references, junk, is_stream)) in objects.iter().enumerate() {
                bytes.extend_from_slice(format!("{} 0 obj [", number + 1).as_bytes());
                for (n, g) in references {
                    bytes.extend_from_slice(format!(" {n} {g} R").as_bytes());
                    expected.insert((*n, *g));
                }
                bytes.extend_from_slice(b" ]");
                if *is_stream {
                    bytes.extend_from_slice(b" stream\n");
                    bytes.extend_from_slice(junk);
                    bytes.extend_from_slice(b"\nendstream");
                }
                bytes.extend_from_slice(b" endobj\n");
                bytes.extend_from_slice(junk);
                bytes.push(b'\n');
            }
            let found = file(&bytes);
            prop_assert_eq!(found.irregular, None);
            prop_assert_eq!(found.references, expected);
        }

        #[test]
        fn arbitrary_bytes_never_panic_and_never_exceed_the_caps(
            bytes in proptest::collection::vec(any::<u8>(), 0..2048),
            count in -2_i64..40,
            first in -2_i64..64,
        ) {
            let found = file(&bytes);
            prop_assert!(found.references.len() <= MAX_DISTINCT_REFERENCES);
            let found = object_stream(&bytes, count, first);
            prop_assert!(found.stream_objects.is_empty());
        }
    }
}
