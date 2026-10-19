//! A page's graphics states are kept whole, and one it does not use that could carry content
//! refuses the split (owner, 2026-10-19; ADR 0019 §2b's graphics-state amendment).
//!
//! # Why `/ExtGState` is not pruned like the other categories
//!
//! Pruning keeps what split's own reading of a page's content says the page uses. Where that
//! reading and another reader's differ, a graphics state the other reader applies is pruned off,
//! and a graphics state is what can hide content -- so the content it hid is drawn in the part.
//! Keeping every page-level graphics state takes that decision away from the reading.
//!
//! **Except where the same object is also another category.** This module does not prune
//! `/ExtGState`, but the other categories are pruned by editing their dictionaries in place, so a
//! kept graphics-state dictionary, or one of its entries, that another kept page lists as a
//! different resource category is edited by that category's pruning, and is not whole. That case
//! is open until pruning is done per object across categories (the release after this rule's).
//!
//! **And split's reading errs both ways.** An entry read as unused is judged below and may refuse.
//! An entry whose name appears anywhere in a kept page's content is read as used, whether or not
//! the page draws with it, and then travels whole with whatever it reaches, without being judged.
//! That too is open and filed (ADR 0019 §2b's graphics-state amendment).
//!
//! # Why keeping them whole needs a refusal beside it
//!
//! A graphics state can reach content of its own: a soft mask's group form, a transfer, black
//! generation or undercolour-removal function, a halftone, a font. Kept whole, an entry only an
//! excluded page draws with would carry that content into a part -- ADR 0019 §2a row 1 reopened.
//! So an entry the kept pages' reading did **not** see used must be content-free, or the split
//! refuses, `[split-graphics-state-unused]`. No reachability is computed: an unused entry that
//! could carry content refuses whoever else reaches it, so there is no identity to get wrong.
//!
//! # Pooled by the dictionary, not by the page
//!
//! Two kept pages may share one `/ExtGState` dictionary, each drawing with a different entry. The
//! used names are unioned across every kept page that shares the dictionary before any entry is
//! judged; judging per page would refuse a part whose pages between them use everything. The pool
//! is the `/ExtGState` dictionary's own identity when it is an indirect object, else the identity
//! of the indirect `/Resources` holding it, else the page alone.
//!
//! # Content-free is an allowlist
//!
//! A key is content-free only with a **direct** value of the listed type; every other key, every
//! indirect value and every value of another type is content-carrying. The spec's default names
//! (`/Default`, `/Identity`) are accepted where they stand for "no function": a document produced
//! by Distiller writes them on graphics states it shares between pages.
//!
//! | key | accepted value |
//! |---|---|
//! | `LW` `ML` `FL` `SM` `CA` `ca` `OPM` `LC` `LJ` | a number |
//! | `OP` `op` `SA` `AIS` `TK` | a boolean |
//! | `Type` | `/ExtGState` |
//! | `RI` | one of the four rendering intents |
//! | `BM` | one of the sixteen blend modes or `/Compatible`, or an array of them |
//! | `D` | `[ [numbers] number ]` |
//! | `SMask` | `/None` |
//! | `TR` | `/Identity` |
//! | `TR2` | `/Identity` or `/Default` |
//! | `BG2` `UCR2` `HT` | `/Default` |
//!
//! # What still travels, accepted (owner, 2026-10-19)
//!
//! An unused content-free entry travels whole, so its key name and its numbers, booleans and names
//! reach the part. That is the residual ADR 0019 §2b's amendment records.

use std::collections::{BTreeMap, BTreeSet};

use burrow_types::{Deadline, Error, Result};

use super::dict_key;
use super::graph::ObjectGraph;
use crate::codes::qpdf::object_type;
use crate::pdfsyntax::top_level_keys;

/// The category this module keeps whole.
pub(super) const EXTGSTATE_KEY: &[u8] = b"/ExtGState";

/// The blend modes PDF 32000-1 Table 136 names, and `/Compatible`, which PDF 1.4 readers accept.
const BLEND_MODES: &[&[u8]] = &[
    b"/Normal",
    b"/Compatible",
    b"/Multiply",
    b"/Screen",
    b"/Overlay",
    b"/Darken",
    b"/Lighten",
    b"/ColorDodge",
    b"/ColorBurn",
    b"/HardLight",
    b"/SoftLight",
    b"/Difference",
    b"/Exclusion",
    b"/Hue",
    b"/Saturation",
    b"/Color",
    b"/Luminosity",
];

/// The four rendering intents PDF 32000-1 §8.6.5.8 names.
const RENDERING_INTENTS: &[&[u8]] = &[
    b"/AbsoluteColorimetric",
    b"/RelativeColorimetric",
    b"/Saturation",
    b"/Perceptual",
];

/// The type of value an allowlisted key may hold, directly.
#[derive(Clone, Copy)]
enum Accepted {
    /// An integer or a real.
    Number,
    /// `true` or `false`.
    Boolean,
    /// One of these names.
    NameIn(&'static [&'static [u8]]),
    /// A name from [`BLEND_MODES`], or an array of them.
    BlendMode,
    /// A line dash pattern: `[ [numbers] number ]`.
    Dash,
}

/// The allowlist: each content-free key, as written in a refusal, and the one shape of value it
/// may have.
const ALLOWLIST: &[(&[u8], &str, Accepted)] = &[
    (b"LW", "/LW", Accepted::Number),
    (b"ML", "/ML", Accepted::Number),
    (b"FL", "/FL", Accepted::Number),
    (b"SM", "/SM", Accepted::Number),
    (b"CA", "/CA", Accepted::Number),
    (b"ca", "/ca", Accepted::Number),
    (b"OPM", "/OPM", Accepted::Number),
    (b"LC", "/LC", Accepted::Number),
    (b"LJ", "/LJ", Accepted::Number),
    (b"OP", "/OP", Accepted::Boolean),
    (b"op", "/op", Accepted::Boolean),
    (b"SA", "/SA", Accepted::Boolean),
    (b"AIS", "/AIS", Accepted::Boolean),
    (b"TK", "/TK", Accepted::Boolean),
    (b"Type", "/Type", Accepted::NameIn(&[b"/ExtGState"])),
    (b"RI", "/RI", Accepted::NameIn(RENDERING_INTENTS)),
    (b"BM", "/BM", Accepted::BlendMode),
    (b"D", "/D", Accepted::Dash),
    (b"SMask", "/SMask", Accepted::NameIn(&[b"/None"])),
    (b"TR", "/TR", Accepted::NameIn(&[b"/Identity"])),
    (
        b"TR2",
        "/TR2",
        Accepted::NameIn(&[b"/Identity", b"/Default"]),
    ),
    (b"BG2", "/BG2", Accepted::NameIn(&[b"/Default"])),
    (b"UCR2", "/UCR2", Accepted::NameIn(&[b"/Default"])),
    (b"HT", "/HT", Accepted::NameIn(&[b"/Default"])),
];

/// What makes one graphics state content-carrying, named at rule level for the refusal.
enum Carrying {
    /// The entry is not a dictionary (a stream, an array, anything else).
    NotADictionary,
    /// An allowlisted key whose value is indirect or of another shape. The key is the spec's.
    Value(&'static str),
    /// A key the allowlist does not name. Not echoed: the document chose it.
    UnknownKey,
}

/// One `/ExtGState` dictionary, and its entries no kept page sharing it has been read to use.
///
/// **The dictionary's own names, not the pages'.** A page's used-name set can hold tens of
/// thousands of names, and keeping one per page until every page is read would hold the sum of
/// them; this holds at most the dictionary's own entries, which the input bounds.
struct Pool<H> {
    states: H,
    /// Each entry's name, and whether a kept page sharing the dictionary draws with it.
    entries: Vec<(Vec<u8>, bool)>,
}

/// Every kept page's `/ExtGState` dictionary, each pooled with the pages that share it.
///
/// The pool is the `/ExtGState` dictionary's own identity when it is an indirect object; else the
/// identity of the indirect `/Resources` holding it; else the page alone, which shares it with
/// nobody. Two maps rather than one keyed by an enum, because a map of this key type is already in
/// the build and a third kind of map is not free on the web.
pub(super) struct Pools<H> {
    pools: Vec<Pool<H>>,
    by_dictionary: BTreeMap<(i32, i32), usize>,
    by_resources: BTreeMap<(i32, i32), usize>,
}

impl<H> Pools<H> {
    pub(super) fn new() -> Self {
        Self {
            pools: Vec::new(),
            by_dictionary: BTreeMap::new(),
            by_resources: BTreeMap::new(),
        }
    }

    /// Record a kept page's `/ExtGState`, if it has one, with the names the page was read to use.
    ///
    /// The deadline is read at every entry of the dictionary this visits.
    ///
    /// # Errors
    ///
    /// The deadline, and whatever the engine reports.
    pub(super) fn add<G: ObjectGraph<Handle = H>>(
        &mut self,
        graph: &G,
        resources: &H,
        used: &BTreeSet<Vec<u8>>,
        deadline: &Deadline,
        clock: &dyn burrow_types::Clock,
    ) -> Result<()> {
        let states = lookup(graph, resources, EXTGSTATE_KEY)?;
        if shape(graph, &states)?.0 != object_type::DICTIONARY {
            // Absent, or not a dictionary -- which `prune_resource_dictionary` refuses for every
            // category, `[split-resource-category-not-a-dictionary]`.
            return Ok(());
        }
        let own = identity(graph, &states)?;
        let holder = identity(graph, resources)?;
        let (index, identity) = if own != (0, 0) {
            (Some(&mut self.by_dictionary), own)
        } else if holder != (0, 0) {
            (Some(&mut self.by_resources), holder)
        } else {
            // A direct `/ExtGState` in a direct `/Resources`: this page's alone, never pooled.
            (None, (0, 0))
        };
        let known = index.as_ref().and_then(|map| map.get(&identity).copied());
        let pool = match known {
            Some(pool) => pool,
            None => {
                let mut entries = Vec::new();
                for name in keys(graph, &states)? {
                    tick(deadline, clock)?;
                    entries.push((name, false));
                }
                let pool = self.pools.len();
                if let Some(map) = index {
                    map.insert(identity, pool);
                }
                self.pools.push(Pool { states, entries });
                pool
            }
        };
        let Some(pool) = self.pools.get_mut(pool) else {
            return Err(Error::Internal(
                "a graphics-state pool the index names is missing".to_owned(),
            ));
        };
        for (name, drawn) in &mut pool.entries {
            if *drawn {
                continue;
            }
            tick(deadline, clock)?;
            // A kept page draws with it: it is theirs, content and all.
            *drawn = used.contains(name);
        }
        Ok(())
    }

    /// Refuse if any pooled dictionary holds an entry no page sharing it uses that could carry
    /// content.
    ///
    /// The deadline is read at every entry and at every array item, so a dictionary of many
    /// entries, or a dash array of many numbers, is inside `max_duration_ms`.
    ///
    /// # Errors
    ///
    /// `[split-graphics-state-unused]`, the deadline, and whatever the engine reports.
    pub(super) fn refuse_unused<G: ObjectGraph<Handle = H>>(
        &self,
        graph: &G,
        deadline: &Deadline,
        clock: &dyn burrow_types::Clock,
    ) -> Result<()> {
        for pool in &self.pools {
            for (name, _) in pool.entries.iter().filter(|(_, drawn)| !drawn) {
                tick(deadline, clock)?;
                let entry = lookup(graph, &pool.states, name)?;
                if let Some(why) = carrying(graph, &entry, deadline, clock)? {
                    return Err(unused_refusal(&why));
                }
            }
        }
        Ok(())
    }
}

/// Why `entry` could carry content, or `None` if every key in it is on the allowlist with a direct
/// value of its listed shape.
///
/// # Every engine read goes through one of the helpers below
///
/// On the web each read is a bridge call that the optimiser would otherwise inline at every site;
/// funnelling them through a handful of functions that are never inlined is most of what keeps
/// this rule's cost in the base payload small.
fn carrying<G: ObjectGraph>(
    graph: &G,
    entry: &G::Handle,
    deadline: &Deadline,
    clock: &dyn burrow_types::Clock,
) -> Result<Option<Carrying>> {
    if shape(graph, entry)?.0 != object_type::DICTIONARY {
        return Ok(Some(Carrying::NotADictionary));
    }
    for key in keys(graph, entry)? {
        tick(deadline, clock)?;
        let Some((_, spelled, accepted)) = ALLOWLIST
            .iter()
            .find(|(allowed, _, _)| *allowed == key.as_slice())
        else {
            return Ok(Some(Carrying::UnknownKey));
        };
        let value = lookup(graph, entry, &key)?;
        if !accepts(graph, &value, *accepted, deadline, clock)? {
            return Ok(Some(Carrying::Value(spelled)));
        }
    }
    Ok(None)
}

/// Whether `value` is direct and of the `accepted` shape.
fn accepts<G: ObjectGraph>(
    graph: &G,
    value: &G::Handle,
    accepted: Accepted,
    deadline: &Deadline,
    clock: &dyn burrow_types::Clock,
) -> Result<bool> {
    // DIRECT, ALWAYS. An indirect value is an object the excluded pages may share and the writer
    // emits on its own; a number behind a reference is not what the allowlist accepts.
    let (kind, direct) = shape(graph, value)?;
    if !direct {
        return Ok(false);
    }
    Ok(match accepted {
        Accepted::Number => is_number(kind),
        Accepted::Boolean => kind == object_type::BOOLEAN,
        Accepted::NameIn(names) => name_in(graph, value, names)?,
        Accepted::BlendMode => match kind {
            object_type::NAME => name_in(graph, value, BLEND_MODES)?,
            object_type::ARRAY => every_item(graph, value, Item::BlendMode, deadline, clock)?,
            _ => false,
        },
        Accepted::Dash => {
            kind == object_type::ARRAY && length(graph, value)? == 2 && {
                let dashes = item(graph, value, 0)?;
                let phase = item(graph, value, 1)?;
                shape(graph, &dashes)? == (object_type::ARRAY, true)
                    && every_item(graph, &dashes, Item::Number, deadline, clock)?
                    && direct_number(graph, &phase)?
            }
        }
    })
}

/// What every item of an array must be.
#[derive(Clone, Copy)]
enum Item {
    /// A direct number.
    Number,
    /// A direct name from [`BLEND_MODES`].
    BlendMode,
}

/// Whether every item of the array `of` is `want`, reading the deadline at each.
fn every_item<G: ObjectGraph>(
    graph: &G,
    of: &G::Handle,
    want: Item,
    deadline: &Deadline,
    clock: &dyn burrow_types::Clock,
) -> Result<bool> {
    for at in 0..length(graph, of)? {
        tick(deadline, clock)?;
        let item = item(graph, of, at)?;
        let fits = match want {
            Item::Number => direct_number(graph, &item)?,
            Item::BlendMode => shape(graph, &item)?.1 && name_in(graph, &item, BLEND_MODES)?,
        };
        if !fits {
            return Ok(false);
        }
    }
    Ok(true)
}

const fn is_number(kind: i32) -> bool {
    kind == object_type::INTEGER || kind == object_type::REAL
}

fn direct_number<G: ObjectGraph>(graph: &G, value: &G::Handle) -> Result<bool> {
    let (kind, direct) = shape(graph, value)?;
    Ok(direct && is_number(kind))
}

/// Read the deadline. One function rather than the check at each site, for the reason above.
#[inline(never)]
fn tick(deadline: &Deadline, clock: &dyn burrow_types::Clock) -> Result<()> {
    deadline.checkpoint(clock)
}

/// This value's type, and whether it is written directly rather than behind a reference.
#[inline(never)]
fn shape<G: ObjectGraph>(graph: &G, value: &G::Handle) -> Result<(i32, bool)> {
    Ok((graph.type_code(value)?, graph.identity(value)? == (0, 0)))
}

/// Whether `value` is a name, and one of `names`.
#[inline(never)]
fn name_in<G: ObjectGraph>(graph: &G, value: &G::Handle, names: &[&[u8]]) -> Result<bool> {
    if graph.type_code(value)? != object_type::NAME {
        return Ok(false);
    }
    Ok(names.contains(&graph.name(value)?.as_slice()))
}

#[inline(never)]
fn identity<G: ObjectGraph>(graph: &G, value: &G::Handle) -> Result<(i32, i32)> {
    graph.identity(value)
}

/// The value at `name` (with or without its `/`) in the dictionary `of`.
#[inline(never)]
fn lookup<G: ObjectGraph>(graph: &G, of: &G::Handle, name: &[u8]) -> Result<G::Handle> {
    graph.key(of, &dict_key(name)?)
}

/// The dictionary `of`'s keys, without their `/`.
#[inline(never)]
fn keys<G: ObjectGraph>(graph: &G, of: &G::Handle) -> Result<Vec<Vec<u8>>> {
    top_level_keys(&graph.unparse(of)?)
}

#[inline(never)]
fn length<G: ObjectGraph>(graph: &G, of: &G::Handle) -> Result<i32> {
    graph.array_len(of)
}

#[inline(never)]
fn item<G: ObjectGraph>(graph: &G, of: &G::Handle, at: i32) -> Result<G::Handle> {
    graph.array_item(of, at)
}

fn unused_refusal(why: &Carrying) -> Error {
    let what = match why {
        Carrying::NotADictionary => "a graphics state that is not a dictionary".to_owned(),
        Carrying::Value(key) => format!("a graphics state whose {key} could carry content"),
        Carrying::UnknownKey => "a graphics state with a key outside the allowlist".to_owned(),
    };
    Error::Unsupported(format!(
        "split [split-graphics-state-unused]: a page lists {what} that the pages of this part do \
         not draw with, so it may belong to a page outside the part (ADR 0019 §2b)"
    ))
}

/// A resource category written as something other than a dictionary.
///
/// Readers resolve resources from such a category, and pruning cannot filter it, so it can carry
/// whatever an excluded page draws with into a part.
pub(super) fn category_not_a_dictionary() -> Error {
    Error::Unsupported(
        "split [split-resource-category-not-a-dictionary]: a page's resources write a category \
         as something other than a dictionary, which split cannot filter to what the part uses \
         (ADR 0019 §2b)"
            .to_owned(),
    )
}
