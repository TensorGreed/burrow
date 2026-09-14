//! Reading PDF syntax in Rust, for the two questions qpdf will not answer safely.
//!
//! Pure Rust, `forbid(unsafe_code)`, no allocation in proportion to its input beyond the answer it
//! returns. Like [`crate::prescan`] it is compiled everywhere rather than gated on the engines,
//! because the web path needs exactly the same two answers and a second implementation of a
//! tokeniser is a second thing to get wrong.
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

pub mod dict;
pub mod names;

pub use dict::top_level_keys;
pub use names::names_in_content;
