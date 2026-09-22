//! Reading and rewriting PDF syntax in Rust, for the questions qpdf will not answer safely.
//!
//! Pure Rust, `forbid(unsafe_code)`. Like [`crate::prescan`] it is compiled everywhere rather than
//! gated on the engines, because the web path needs exactly the same answers and a second
//! implementation of a tokeniser is a second thing to get wrong.
//!
//! # What it allocates, corrected
//!
//! This header used to say "no allocation in proportion to its input beyond the answer it
//! returns". That was true of [`dict`] and [`names`], whose answers are a key list and a name set,
//! and #128 made it false: [`ops`] returns one [`Operation`] per operator with its operands, which
//! is **several times the input** rather than a summary of it, and [`contents`] copies its input
//! outright to concatenate it. Both carry their own ceilings — [`ops::MAX_OPERATIONS`],
//! [`ops::MAX_TOTAL_OPERANDS`], [`ops::MAX_OPERAND_BYTES`] and [`contents::MAX_ELEMENTS`] — and
//! each is a refusal rather than a truncation. The sentence is corrected rather than deleted
//! because the earlier version of `ops` had no aggregate bound at all, and it was this claim,
//! left standing, that made that look deliberate.
//!
//! # Why this exists at all, rather than asking the engine
//!
//! Both questions have a qpdf entry point, and burrow may call neither.
//!
//! | question | qpdf's answer | why it is unavailable |
//! |---|---|---|
//! | a dictionary's keys | `qpdf_oh_begin_dict_key_iter` + `_dict_more_keys` + `_dict_next_key` | none of the three reaches `trap_errors`; the latter two do not go near it (ADR 0013 §1) |
//! | a content stream's resource operands | `QPDFObjectHandle::parseContentStream` | not in the C API at all |
//!
//! What *is* available is `qpdf_oh_unparse` and `qpdf_oh_get_page_content_data`, both trapped, both
//! handing back bytes. So qpdf does the parsing that needs the document — resolving objects,
//! decoding streams — and this does the tokenising that does not.
//!
//! A third question arrived with M2 and has no qpdf answer either: **where in the bytes each
//! operator and operand is**, so a page's content can be rewritten rather than only read.
//! `QPDFObjectHandle::parseContentStream` is not in the C API, and `qpdf_oh_get_page_content_data`
//! hands back a page's streams *concatenated*, losing which element each byte came from. [`ops`]
//! and [`contents`] are those two halves. ADR 0029 §4.
//!
//! # What uses it, and what M2 inherits
//!
//! `split`'s pruning (ADR 0019 §2b, issue #54) needs both answers: [`dict::top_level_keys`] to make
//! the page-key rule an **allowlist** rather than a list of keys somebody thought of, and
//! [`names::names_in_content`] because pruning an inherited `/Resources` "is not a dictionary
//! filter" — a resource is referenced from a page's content stream, a nested Form XObject's, a
//! tiling pattern's, a Type 3 `/CharProcs` entry and an annotation's appearance stream.
//!
//! **Redaction reuses this module unchanged.** It is the part of the pruning machinery with no
//! engine in it and no notion of what a page is, so it transfers whole; see ADR 0019's section on
//! what carries to M2 and what is `split`-only.
//!
//! # The two failure directions are not symmetric, and each module says which way it leans
//!
//! [`names`] over-approximates — an extra name keeps a resource that could have gone, where a
//! missing one deletes a resource the page draws with. [`dict`] may not approximate in either
//! direction: a key it fails to report is a key that is never removed, which is the leak. Both
//! refuse rather than return a partial answer, and their rustdoc says why in each case.

#![forbid(unsafe_code)]

mod lexer;

pub mod contents;
pub mod dict;
pub mod geometry;
pub mod names;
pub mod ops;
pub mod strings;

pub use contents::{Contents, Edit};
pub use dict::top_level_keys;
pub use names::names_in_content;
pub use ops::{Operand, Operation, Span, operations};
pub use strings::{decode_string, encode_literal};
