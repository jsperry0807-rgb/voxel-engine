//! Key-based resolution at the persistence boundary.
//!
//! A chunk holds [`MaterialId`]s, and a [`MaterialId`] only means something against
//! the registry that handed it out: ids are assigned in registration order,
//! so a session that registers the same blocks in a different order numbers
//! them differently. A file that outlives a session therefore cannot carry
//! ids — it carries [`NamespacedKey`]s, and the loader turns them back into
//! ids when it loads.
//!
//! This module is that turn: the [`Palette`] type (keys, in order), the
//! [`MissingBlockPolicy`] that says what to do about a block this session
//! does not have, and the resolution itself.
//!
//! # The three rules
//!
//! - **Parse, don't validate.** A [`Palette`] can only be built from valid
//!   keys, and [`Palette::parse`] either returns a well-formed palette or an
//!   error — never a palette holding half a key.
//! - **Total, not partial.** [`BlockRegistry::resolve_palette`] returns a
//!   [`Resolution`] or a typed [`PaletteError`] for *every* input. It never
//!   panics and never silently substitutes: a file naming a block this
//!   session lacks is either an error or an explicitly reported omission.
//!   Compare [`BlockRegistry::resolved`], which is *partial* — it panics on
//!   an unknown id, because it only ever sees ids the registry itself
//!   produced.
//! - **Palette remapping.** Resolution is by key, so a palette written by one
//!   session loads correctly into another, whatever order either registered
//!   its blocks in.
//!
//! # Textures
//!
//! Texture ids are session-local for the same reason material ids are, so they
//! are *derived*, never persisted: the registry resolves texture keys to
//! texture ids once, at [`BlockRegistryBuilder::freeze`](crate::BlockRegistryBuilder::freeze),
//! and a file that names a block gets the textures that block has in *this*
//! session. The save format therefore never contains a texture id either —
//! see `docs/save-format.md`.

use core::fmt;

use crate::id::{KeyError, MaterialId, NamespacedKey};
use crate::registry::BlockRegistry;

/// Where a key was referenced from.
///
/// The context travels with the error so a loader can say *which* entry of
/// *which* list was missing, instead of "some block was missing".
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KeyContext {
    /// An entry of a palette, by its index in the palette.
    Palette {
        /// The palette index of the entry.
        index: usize,
    },
    /// A member of a named tag.
    Tag {
        /// The tag the member was listed under.
        tag: NamespacedKey,
    },
}

impl fmt::Display for KeyContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyContext::Palette { index } => write!(f, "palette index {index}"),
            KeyContext::Tag { tag } => write!(f, "tag `{tag}`"),
        }
    }
}

/// A key that this session's registry cannot resolve.
///
/// Carries the key itself and where it was referenced from, so a caller can
/// report *which* block is missing and *where* it was named — the difference
/// between "a save will not load" and "the save will not load because
/// `mymod:copper_ore` is gone, and it was in `mymod:mineable/pickaxe`".
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MissingKey {
    key: NamespacedKey,
    context: KeyContext,
}

impl MissingKey {
    /// Builds a missing-key report.
    pub fn new(key: NamespacedKey, context: KeyContext) -> Self {
        Self { key, context }
    }

    /// The key that could not be resolved.
    pub fn key(&self) -> &NamespacedKey {
        &self.key
    }

    /// Where the key was referenced from.
    pub fn context(&self) -> &KeyContext {
        &self.context
    }
}

impl fmt::Display for MissingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` (from {})", self.key, self.context)
    }
}

/// What to do about a block a persisted file names but this session does not
/// have.
///
/// A file outlives the session that wrote it, so "the block is not installed
/// any more" is a normal outcome, not a bug — and each caller wants a
/// different answer to it. The choice is the caller's, made explicitly at
/// the boundary; the boundary never decides it silently.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MissingBlockPolicy {
    /// Fail the load.
    ///
    /// The strict option: a file that references a block this session lacks
    /// is refused outright. Right when the file is a save the player expects
    /// to be exactly what they left behind.
    Error,
    /// Stand another block in for the missing one.
    ///
    /// The substitute must be a block this registry has; an id outside the
    /// registry's dense id space is rejected as
    /// [`PaletteError::InvalidSubstitute`] rather than written into a chunk
    /// where it would resolve to nonsense.
    Substitute(MaterialId),
    /// Leave the entry out entirely.
    ///
    /// Only meaningful where the list is a *set* (a tag's membership), never
    /// for a palette: a palette is index-addressed, and dropping an entry
    /// would shift every index after it. A palette resolved under this policy
    /// is refused as [`PaletteError::UnsupportedPolicy`].
    Drop,
}

/// Everything that can go wrong at the palette boundary.
///
/// Every variant is *reported* — none of them is a panic, and none of them
/// is a silent substitution.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PaletteError {
    /// A line of the palette was not a valid namespaced key.
    ///
    /// A bare number is the usual culprit: an id written where a key was
    /// required. It cannot even be represented, because a key's namespace is
    /// part of its identity.
    InvalidKey {
        /// The 1-based line number the bad entry was on.
        line: usize,
        /// The text of the offending line.
        raw: String,
        /// Why the text is not a key.
        source: KeyError,
    },
    /// A referenced block is not registered in this registry.
    MissingBlock(MissingKey),
    /// The policy's stand-in block is not registered in this registry.
    InvalidSubstitute {
        /// The id the policy named.
        id: MaterialId,
    },
    /// The policy cannot be applied to an index-addressed palette.
    UnsupportedPolicy {
        /// The policy that was asked for.
        policy: MissingBlockPolicy,
    },
}

impl fmt::Display for PaletteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaletteError::InvalidKey { line, raw, source } => {
                write!(f, "line {line} (`{raw}`) is not a block key: {source}")
            }
            PaletteError::MissingBlock(missing) => {
                write!(f, "block {} is not registered", missing)
            }
            PaletteError::InvalidSubstitute { id } => {
                write!(f, "substitute block {id} is not registered")
            }
            PaletteError::UnsupportedPolicy { policy } => match policy {
                MissingBlockPolicy::Drop => write!(
                    f,
                    "a palette is index-addressed, so entries cannot be dropped"
                ),
                other => write!(f, "policy {other:?} cannot be applied to a palette"),
            },
        }
    }
}

impl std::error::Error for PaletteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PaletteError::InvalidKey { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A key that could not be resolved, and what happened to it.
///
/// Every unresolved key is recorded, so a loader can report *all* of the
/// missing blocks at once rather than one per reload.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Omission {
    key: NamespacedKey,
    context: KeyContext,
    replacement: Option<MaterialId>,
}

impl Omission {
    /// The key that could not be resolved.
    pub fn key(&self) -> &NamespacedKey {
        &self.key
    }

    /// Where the key was referenced from.
    pub fn context(&self) -> &KeyContext {
        &self.context
    }

    /// The id that stood in for the missing block, if one did.
    ///
    /// `None` means the entry was dropped rather than replaced.
    pub fn replacement(&self) -> Option<MaterialId> {
        self.replacement
    }

    /// True if the entry was dropped instead of replaced.
    pub fn is_dropped(&self) -> bool {
        self.replacement.is_none()
    }
}

/// The outcome of resolving keys into this session's ids.
///
/// The ids are in the same order as the keys they came from — which, for a
/// palette, means `ids[i]` is the block that palette index `i` names.
///
/// Under [`MissingBlockPolicy::Drop`] a key that did not resolve leaves the
/// list, so the ids are shorter than the keys. Every such key is recorded in
/// [`Resolution::omissions`], so a caller can always say what went missing.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Resolution {
    ids: Vec<MaterialId>,
    omissions: Vec<Omission>,
}

impl Resolution {
    /// The resolved ids, in the order of the keys they came from.
    pub fn ids(&self) -> &[MaterialId] {
        &self.ids
    }

    /// Takes the resolved ids.
    pub fn into_ids(self) -> Vec<MaterialId> {
        self.ids
    }

    /// The keys that could not be resolved, and what happened to each.
    pub fn omissions(&self) -> &[Omission] {
        &self.omissions
    }

    /// True if every key resolved.
    pub fn is_complete(&self) -> bool {
        self.omissions.is_empty()
    }

    /// Number of resolved ids.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// True if nothing resolved.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// An ordered list of block identities, stored as keys.
///
/// This is the persisted shape of "which blocks does this file use": the
/// chunk data refers to palette *indices*, and the palette says what each
/// index means.
///
/// It can only be built from keys. There is no constructor that takes a
/// [`MaterialId`], because a session id means nothing outside the session that
/// produced it — the "keys in, ids never" rule is enforced by the type
/// rather than by convention.
///
/// # Duplicates
///
/// Two entries may name the same block. That is harmless: both indices
/// resolve to the same id. Rejecting duplicates would be a *quality* check
/// rather than a validity one, and a palette with a duplicate is still a
/// well-formed list of keys.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Palette {
    keys: Vec<NamespacedKey>,
}

impl Palette {
    /// Builds a palette from keys.
    pub fn new(keys: Vec<NamespacedKey>) -> Self {
        Self { keys }
    }

    /// Parses the canonical palette encoding.
    ///
    /// One key per line. Blank lines and lines whose first non-space
    /// character is `#` are ignored, so a palette can carry comments. Every
    /// other line must be a valid namespaced key — anything else (a bare
    /// number, a key with the wrong case, a path with a space) is reported
    /// as [`PaletteError::InvalidKey`] with its line number.
    ///
    /// This is the encoding the save format's palette section uses, and
    /// `docs/save-format.md` is its contract.
    pub fn parse(text: &str) -> Result<Self, PaletteError> {
        let mut keys = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let key = NamespacedKey::parse(line).map_err(|source| PaletteError::InvalidKey {
                line: number + 1,
                raw: line.to_string(),
                source,
            })?;
            keys.push(key);
        }
        Ok(Self { keys })
    }

    /// The keys in the palette, in order.
    pub fn keys(&self) -> &[NamespacedKey] {
        &self.keys
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True if the palette is empty.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl fmt::Display for Palette {
    /// Writes the canonical encoding: one key per line, no comments.
    ///
    /// The output re-parses to an equal palette, so a palette can round-trip
    /// through text without losing anything.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for key in &self.keys {
            writeln!(f, "{key}")?;
        }
        Ok(())
    }
}

impl BlockRegistry {
    /// Resolves a persisted palette into this session's ids.
    ///
    /// This is the load path's whole job: the palette names blocks by key,
    /// and this turns them into the ids this session's chunks store. A
    /// palette written by a session that registered its blocks in a
    /// different order loads correctly, because the mapping goes through
    /// keys rather than through numbers.
    ///
    /// `ids[i]` is the block that palette index `i` names — the index
    /// correspondence is what lets chunk data be decoded against the result.
    ///
    /// # Errors
    ///
    /// - [`PaletteError::MissingBlock`] if a key names a block this registry
    ///   does not have and the policy is [`MissingBlockPolicy::Error`]. The
    ///   first missing key in palette order is reported.
    /// - [`PaletteError::InvalidSubstitute`] if the policy names a
    ///   substitute this registry does not have.
    /// - [`PaletteError::UnsupportedPolicy`] if the policy is
    ///   [`MissingBlockPolicy::Drop`], which a palette cannot honour.
    pub fn resolve_palette(
        &self,
        palette: &Palette,
        policy: MissingBlockPolicy,
    ) -> Result<Resolution, PaletteError> {
        if policy == MissingBlockPolicy::Drop {
            // A palette is index-addressed: chunk data refers to entries by
            // position, so an entry cannot be removed without renumbering
            // every entry after it. Refusing the policy is the only honest
            // answer — silently renumbering would corrupt every chunk.
            return Err(PaletteError::UnsupportedPolicy { policy });
        }

        resolve_keys(self, &palette.keys, policy, |index| KeyContext::Palette {
            index,
        })
    }

    /// Resolves the member keys of a tag into this session's ids.
    ///
    /// Tags are named by key and name their members by key; this is where
    /// those member keys become ids, at the boundary where a data file is
    /// read. A tag is a *set*, so [`MissingBlockPolicy::Drop`] is allowed
    /// here: a member this session does not have is simply not a member.
    ///
    /// The result feeds straight into [`crate::BlockTags::insert_all`].
    ///
    /// # Errors
    ///
    /// - [`PaletteError::MissingBlock`] if a key names a block this registry
    ///   does not have and the policy is [`MissingBlockPolicy::Error`].
    /// - [`PaletteError::InvalidSubstitute`] if the policy names a
    ///   substitute this registry does not have.
    pub fn resolve_tag_members(
        &self,
        tag: &NamespacedKey,
        members: &[NamespacedKey],
        policy: MissingBlockPolicy,
    ) -> Result<Resolution, PaletteError> {
        resolve_keys(self, members, policy, |_| KeyContext::Tag {
            tag: tag.clone(),
        })
    }
}

/// Resolves keys to ids under a policy, recording every omission.
///
/// The shared body of [`BlockRegistry::resolve_palette`] and
/// [`BlockRegistry::resolve_tag_members`]: both are "a list of keys becomes a
/// list of ids", and they differ only in what they say when a key does not
/// resolve.
fn resolve_keys(
    registry: &BlockRegistry,
    keys: &[NamespacedKey],
    policy: MissingBlockPolicy,
    context: impl Fn(usize) -> KeyContext,
) -> Result<Resolution, PaletteError> {
    // Validate the substitute once, up front: it is a caller-supplied id,
    // and an id outside the registry's dense space would otherwise end up
    // written into a chunk where it resolves to nothing.
    if let MissingBlockPolicy::Substitute(id) = policy
        && id.index() >= registry.len()
    {
        return Err(PaletteError::InvalidSubstitute { id });
    }

    let mut ids = Vec::with_capacity(keys.len());
    let mut omissions = Vec::new();

    for (index, key) in keys.iter().enumerate() {
        match registry.id(key) {
            Some(id) => ids.push(id),
            None => match policy {
                MissingBlockPolicy::Error => {
                    return Err(PaletteError::MissingBlock(MissingKey::new(
                        key.clone(),
                        context(index),
                    )));
                }
                MissingBlockPolicy::Substitute(replacement) => {
                    omissions.push(Omission {
                        key: key.clone(),
                        context: context(index),
                        replacement: Some(replacement),
                    });
                    ids.push(replacement);
                }
                MissingBlockPolicy::Drop => {
                    // The entry leaves the list entirely; the omission record
                    // is what keeps the loader able to say so.
                    omissions.push(Omission {
                        key: key.clone(),
                        context: context(index),
                        replacement: None,
                    });
                }
            },
        }
    }

    Ok(Resolution { ids, omissions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BlockKind;
    use crate::registry::BlockRegistryBuilder;

    fn registry_with(kinds: impl IntoIterator<Item = BlockKind>) -> BlockRegistry {
        let mut builder = BlockRegistryBuilder::new();
        for kind in kinds {
            // The builder already pre-registers air at id 0.
            if kind == BlockKind::Air {
                continue;
            }
            builder.register_kind(kind).unwrap();
        }
        builder.freeze().unwrap()
    }

    #[test]
    fn a_palette_is_built_from_keys_and_holds_nothing_else() {
        let palette = Palette::new(vec![BlockKind::Stone.key(), BlockKind::Dirt.key()]);
        assert_eq!(palette.len(), 2);
        assert_eq!(palette.keys()[0], BlockKind::Stone.key());
        assert!(!palette.is_empty());
    }

    #[test]
    fn parse_skips_comments_and_blank_lines() {
        let palette = Palette::parse(
            "# a comment\n\
             \n\
             voxel:stone\n\
             \n\
             # another comment\n\
             voxel:dirt\n",
        )
        .unwrap();

        assert_eq!(palette.len(), 2);
        assert_eq!(palette.keys()[0], BlockKind::Stone.key());
        assert_eq!(palette.keys()[1], BlockKind::Dirt.key());
    }

    #[test]
    fn a_bare_number_is_not_a_key_and_is_reported_with_its_line() {
        // The "ids never" rule, enforced by the parser: an id cannot be
        // represented in a palette at all.
        let err = Palette::parse("voxel:stone\n42\n").unwrap_err();

        match err {
            PaletteError::InvalidKey { line, raw, source } => {
                assert_eq!(line, 2);
                assert_eq!(raw, "42");
                assert_eq!(source, KeyError::MissingSeparator);
            }
            other => panic!("expected an invalid-key error, got {other:?}"),
        }
    }

    #[test]
    fn a_palette_round_trips_through_text() {
        let palette = Palette::new(vec![BlockKind::Stone.key(), BlockKind::Dirt.key()]);
        let text = palette.to_string();
        let reparsed = Palette::parse(&text).unwrap();
        assert_eq!(reparsed, palette);
    }

    #[test]
    fn the_canonical_text_is_one_key_per_line() {
        let palette = Palette::new(vec![BlockKind::Stone.key(), BlockKind::Dirt.key()]);
        assert_eq!(palette.to_string(), "voxel:stone\nvoxel:dirt\n");
    }

    #[test]
    fn duplicate_entries_are_allowed_and_resolve_to_the_same_id() {
        let registry = registry_with([BlockKind::Stone]);
        let palette = Palette::new(vec![BlockKind::Stone.key(), BlockKind::Stone.key()]);

        let resolution = registry
            .resolve_palette(&palette, MissingBlockPolicy::Error)
            .unwrap();

        assert_eq!(resolution.ids().len(), 2);
        assert_eq!(resolution.ids()[0], resolution.ids()[1]);
        assert!(resolution.is_complete());
    }

    #[test]
    fn an_out_of_range_substitute_is_refused_even_when_nothing_is_missing() {
        let registry = registry_with([BlockKind::Stone]);
        let palette = Palette::new(vec![BlockKind::Stone.key()]);

        let err = registry
            .resolve_palette(
                &palette,
                MissingBlockPolicy::Substitute(MaterialId::from_raw(99)),
            )
            .unwrap_err();

        assert_eq!(
            err,
            PaletteError::InvalidSubstitute {
                id: MaterialId::from_raw(99)
            }
        );
    }

    #[test]
    fn a_reserved_slot_is_a_valid_substitute() {
        // A reserved slot is inside the dense id space and draws nothing,
        // which makes it a legitimate stand-in.
        let mut builder = BlockRegistryBuilder::new();
        builder.reserve(1).unwrap();
        let registry = builder.freeze().unwrap();

        let palette = Palette::new(vec![NamespacedKey::new("mymod", "gone").unwrap()]);
        let resolution = registry
            .resolve_palette(
                &palette,
                MissingBlockPolicy::Substitute(MaterialId::from_raw(1)),
            )
            .unwrap();

        assert_eq!(resolution.ids()[0], MaterialId::from_raw(1));
    }

    #[test]
    fn dropping_is_refused_for_a_palette() {
        let registry = registry_with([BlockKind::Stone]);
        let palette = Palette::new(vec![BlockKind::Stone.key()]);

        let err = registry
            .resolve_palette(&palette, MissingBlockPolicy::Drop)
            .unwrap_err();

        assert_eq!(
            err,
            PaletteError::UnsupportedPolicy {
                policy: MissingBlockPolicy::Drop
            }
        );
    }

    #[test]
    fn a_dropped_tag_member_leaves_the_list_and_is_recorded() {
        let registry = registry_with([BlockKind::Stone]);
        let tag = NamespacedKey::new("mymod", "mineable/pickaxe").unwrap();
        let members = vec![
            BlockKind::Stone.key(),
            NamespacedKey::new("mymod", "gone").unwrap(),
        ];

        let resolution = registry
            .resolve_tag_members(&tag, &members, MissingBlockPolicy::Drop)
            .unwrap();

        assert_eq!(resolution.ids().len(), 1);
        assert_eq!(
            resolution.ids()[0],
            registry.id(&BlockKind::Stone.key()).unwrap()
        );
        assert_eq!(resolution.omissions().len(), 1);
        assert!(resolution.omissions()[0].is_dropped());
        assert_eq!(
            resolution.omissions()[0].context(),
            &KeyContext::Tag { tag: tag.clone() }
        );
    }
}
