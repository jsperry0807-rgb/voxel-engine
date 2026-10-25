//! Block identity: dense runtime [`MaterialId`]s and validated [`NamespacedKey`]s.
//!
//! At startup the [`crate::BlockRegistry`] assigns every block a `MaterialId`, a
//! small integer that is cheap to copy and cheap to index with. Ids are dense
//! (`0..len`) so they can index registry-ordered arrays directly, and id `0` is
//! permanently reserved for air.
//!
//! Ids are assigned **in registration order**, so an id is only valid against
//! the registry that handed it out: adding, removing or re-registering a block
//! shifts every id after it. That makes ids the right thing to hold in
//! in-memory chunk arrays — the hot path they exist for — and the wrong thing
//! to persist. Save files and network payloads must carry [`NamespacedKey`]s,
//! or carry ids *alongside* a key palette and remap through it on load.
//!
//! `NamespacedKey`s are the stable, human-readable identity of a block
//! ("`mymod:copper_ore`"). Data files, assets and save games refer to blocks by
//! key; the numeric id is an internal, session-local encoding of that key.

use core::fmt;
use core::str::FromStr;
use std::sync::Arc;

/// Id reserved for air. Every [`crate::BlockRegistry`] contains this block.
pub const AIR_ID: MaterialId = MaterialId(0);

/// Namespace that owns the engine's built-in blocks and textures.
///
/// Every key minted by [`NamespacedKey::builtin`] lives in this namespace, so
/// a mod cannot collide with an engine built-in by accident.
pub const BUILTIN_NAMESPACE: &str = "voxel";

/// Dense runtime index of a registered block.
///
/// Copyable and `Ord`, so it can be stored directly in *in-memory* chunk arrays
/// or used as a map key. The value is meaningless until a registry assigns it,
/// and it is only meaningful against the registry and session that produced it:
/// ids are handed out in registration order, so reordering or adding a block
/// renumbers everything after it.
///
/// `#[repr(transparent)]` guarantees the same layout as `u16`, so storage can
/// reinterpret `&[MaterialId]` as `&[u16]` (and back) without copying element by
/// element. That is an in-memory transcode optimization — network encoding,
/// SIMD-friendly buffers — not a persistence format: the raw number is exactly
/// as stable as the registration order that produced it, which is to say not
/// stable at all across sessions.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MaterialId(u16);

impl MaterialId {
    /// Largest id a registry can produce (`u16` storage bounds a world to
    /// 65 536 block types, which is far beyond practical limits).
    ///
    /// This is the *block* cap. The other one — 65 536 distinct textures per
    /// registry — is the practical ceiling, because a block may name six
    /// distinct textures; see the crate's *Limits* section.
    pub const MAX: MaterialId = MaterialId(u16::MAX);

    /// Builds an id from its raw representation.
    ///
    /// Prefer the id handed out by the registry. This exists for reading back
    /// bytes written this session (see [`MaterialId::as_raw`]), for remapping a
    /// persisted key palette into the current session's ids, and for tests.
    ///
    /// The argument is not validated and carries no authority: `from_raw`
    /// cannot know whether a registry actually contains that many blocks, and
    /// an id read from an older save is only correct if that save registered
    /// its blocks in the same order. Never treat an id decoded from disk or the
    /// network as identity without the [`NamespacedKey`] it stands for.
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    /// Raw representation, for encoding into a `u16` buffer within this
    /// session. Not a stable on-disk format; pair it with the key if it must
    /// outlive the registry that produced it.
    pub const fn as_raw(self) -> u16 {
        self.0
    }

    /// Index into registry-ordered slices (`0..registry.len()`).
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// True for the reserved air id.
    ///
    /// On the hot path (per-voxel checks in meshing and physics) the
    /// comparison is a single `u16` equality against zero, which is why air
    /// lives at id 0 rather than being looked up by key.
    pub const fn is_air(self) -> bool {
        self.0 == AIR_ID.0
    }
}

impl fmt::Display for MaterialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MaterialId({})", self.0)
    }
}

impl fmt::Debug for MaterialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MaterialId({})", self.0)
    }
}

/// A validated `namespace:path` identifier, e.g. "`mymod:copper_ore`".
///
/// The canonical string is stored once, so equality, hashing and ordering are
/// a single string comparison. Both parts are restricted to lowercase ASCII
/// alphanumeric characters plus `_`, `.` and `-`; the path may additionally
/// contain `/` to express directories (`mymod:ores/deep/copper_ore`).
///
/// The type is `Send + Sync` and cheap to clone (`Arc<str>` is a refcount
/// bump), so keys can live in shared, static registries.
///
/// # Why `Hash`/`Eq`/`Ord` are hand-written
///
/// `sep` is a *cache* of the `:` position, not independent state: two keys with
/// the same canonical string are the same key, full stop. The `derive` macros
/// would fold `sep` into `Eq`, `Hash` and `Ord`, so any future constructor that
/// sets `sep` inconsistently (an `*_unchecked` fast path, a deserializer) would
/// produce keys that compare equal via [`NamespacedKey::as_str`] but *not* via
/// `==` — which silently breaks every `HashMap<NamespacedKey, _>` they touch.
/// These impls therefore depend on `raw` alone.
#[derive(Clone)]
pub struct NamespacedKey {
    raw: Arc<str>,
    /// Byte index of the ':' separator inside `raw`. Derived from `raw`;
    /// deliberately excluded from `Hash`/`Eq`/`Ord`.
    ///
    /// `u32`, not `u16`: the field sits in the alignment padding after the
    /// 16-byte `Arc<str>` fat pointer, so widening it changes no struct size
    /// (still 24 bytes on 64-bit) — while a `u16` would silently truncate for
    /// namespaces longer than 65 535 bytes.
    sep: u32,
}

/// Error returned when a [`NamespacedKey`] fails validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The raw string had no `namespace:path` structure.
    MissingSeparator,
    /// The part before ':' was empty.
    EmptyNamespace,
    /// The namespace exceeded `u32::MAX` bytes, so its separator index could
    /// not be represented. (Only reachable with a multi-gigabyte namespace.)
    NamespaceTooLong,
    /// The part after ':' was empty.
    EmptyPath,
    /// The namespace contained a character outside `[a-z0-9_.-]`.
    InvalidNamespaceChar(char),
    /// The path contained a character outside `[a-z0-9_./-]`.
    InvalidPathChar(char),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::MissingSeparator => write!(f, "key is missing the ':' namespace separator"),
            KeyError::EmptyNamespace => write!(f, "key has an empty namespace"),
            KeyError::NamespaceTooLong => {
                write!(f, "namespace exceeds u32::MAX bytes")
            }
            KeyError::EmptyPath => write!(f, "key has an empty path"),
            KeyError::InvalidNamespaceChar(c) => {
                write!(f, "invalid namespace character {c:?}, expected [a-z0-9_.-]")
            }
            KeyError::InvalidPathChar(c) => {
                write!(f, "invalid path character {c:?}, expected [a-z0-9_./-]")
            }
        }
    }
}

impl std::error::Error for KeyError {}

impl NamespacedKey {
    /// Builds a key from its two parts, validating both.
    pub fn new(namespace: &str, path: &str) -> Result<Self, KeyError> {
        validate_namespace(namespace)?;
        validate_path(path)?;

        // Checked, not `as`: a truncating cast here would silently corrupt
        // `namespace()`/`path()` for absurdly long namespaces.
        let sep = u32::try_from(namespace.len()).map_err(|_| KeyError::NamespaceTooLong)?;

        let mut raw = String::with_capacity(namespace.len() + 1 + path.len());
        raw.push_str(namespace);
        raw.push(':');
        raw.push_str(path);
        Ok(Self {
            raw: raw.into_boxed_str().into(),
            sep,
        })
    }

    /// Parses a canonical `"namespace:path"` string.
    pub fn parse(raw: &str) -> Result<Self, KeyError> {
        let (namespace, path) = raw.split_once(':').ok_or(KeyError::MissingSeparator)?;
        Self::new(namespace, path)
    }

    /// Shorthand for the engine's built-in `voxel:` namespace.
    ///
    /// # Panics
    ///
    /// Panics if `path` is not a valid path (lowercase `[a-z0-9_./-]`). This
    /// is checked in release builds too, not just debug: a bad path would
    /// mint a key that [`NamespacedKey::parse`] rejects, silently breaking the
    /// "every key is validated" invariant that registry and tag lookups rely
    /// on. Built-in paths are literals, so a panic here is a compile-time
    /// mistake surfaced early — pass [`NamespacedKey::new`] instead for
    /// runtime or untrusted input.
    pub fn builtin(path: &str) -> Self {
        assert!(
            validate_path(path).is_ok(),
            "invalid built-in block path {path:?}; \
             use NamespacedKey::new for runtime input"
        );
        Self {
            raw: format!("{BUILTIN_NAMESPACE}:{path}")
                .into_boxed_str()
                .into(),
            // `"voxel"` is a 6-byte literal; the bound is checked at every
            // call site of `validate_path`, so no cast or conversion here.
            sep: BUILTIN_NAMESPACE.len() as u32,
        }
    }

    /// The namespace part, before ':'.
    pub fn namespace(&self) -> &str {
        &self.raw[..self.sep as usize]
    }

    /// The path part, after ':'.
    pub fn path(&self) -> &str {
        &self.raw[self.sep as usize + 1..]
    }

    /// The canonical `namespace:path` string.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Address of the shared string allocation, for asserting that clones
    /// alias the same buffer rather than copying it. Test-only.
    #[cfg(test)]
    pub(crate) fn arc_ptr(&self) -> *const str {
        Arc::as_ptr(&self.raw)
    }

    /// Number of live handles to this key's string allocation. Test-only.
    #[cfg(test)]
    pub(crate) fn refcount(&self) -> usize {
        Arc::strong_count(&self.raw)
    }
}

impl fmt::Display for NamespacedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl fmt::Debug for NamespacedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NamespacedKey({})", self.raw)
    }
}

// `sep` is a cache derived from `raw`; it must not participate in identity.
// Derived impls would compare/hash it too, so any future constructor that sets
// it inconsistently would break `HashMap<NamespacedKey, _>` lookups in a way
// that compiles and fails silently at runtime.

impl PartialEq for NamespacedKey {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl Eq for NamespacedKey {}

impl PartialOrd for NamespacedKey {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NamespacedKey {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}

impl std::hash::Hash for NamespacedKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // `Arc<str>`'s `Hash` delegates to the string contents, not the
        // pointer, so equal keys hash equal regardless of allocation.
        self.raw.hash(state);
    }
}

impl FromStr for NamespacedKey {
    type Err = KeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<&str> for NamespacedKey {
    type Error = KeyError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

fn validate_namespace(namespace: &str) -> Result<(), KeyError> {
    if namespace.is_empty() {
        return Err(KeyError::EmptyNamespace);
    }
    for c in namespace.chars() {
        if !matches!(c, 'a'..='z' | '0'..='9' | '_' | '.' | '-') {
            return Err(KeyError::InvalidNamespaceChar(c));
        }
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), KeyError> {
    if path.is_empty() {
        return Err(KeyError::EmptyPath);
    }
    for c in path.chars() {
        if !matches!(c, 'a'..='z' | '0'..='9' | '_' | '.' | '-' | '/') {
            return Err(KeyError::InvalidPathChar(c));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_id_roundtrips_through_raw() {
        let id = MaterialId::from_raw(42);
        assert_eq!(id.as_raw(), 42);
        assert_eq!(id.index(), 42);
        assert_eq!(MaterialId::MAX.as_raw(), u16::MAX);
        assert_eq!(AIR_ID.as_raw(), 0);
    }

    #[test]
    fn sep_widening_to_u32_is_free() {
        // `Arc<str>` is a 16-byte fat pointer (8-byte aligned); `sep` lands in
        // the padding that follows, so widening u16 -> u32 changes no size.
        // If this fails, the layout assumption behind that choice is gone.
        assert_eq!(
            size_of::<NamespacedKey>(),
            24,
            "sep widening should be absorbed by alignment padding"
        );
        assert_eq!(align_of::<NamespacedKey>(), align_of::<Arc<str>>());
    }

    #[test]
    fn long_namespaces_do_not_silently_truncate() {
        // 70 000 bytes passes validation (all chars legal) but exceeds
        // u16::MAX. The old `as u16` cast wrapped the separator index to
        // 4 464, so both accessors returned wrong slices with no error.
        let long = "a".repeat(70_000);
        let key = NamespacedKey::new(&long, "copper_ore").expect("valid key");

        assert_eq!(key.namespace(), long);
        assert_eq!(key.path(), "copper_ore");
        assert_eq!(key.as_str().len(), 70_000 + 1 + "copper_ore".len());

        // And the key still round-trips and hashes consistently.
        assert_eq!(NamespacedKey::parse(key.as_str()).unwrap(), key);
    }

    #[test]
    fn is_air_is_exactly_the_zero_sentinel() {
        assert!(AIR_ID.is_air());
        assert!(MaterialId::from_raw(0).is_air());
        assert!(!MaterialId::from_raw(1).is_air());
        assert!(!MaterialId::MAX.is_air());
    }

    #[test]
    fn material_id_is_layout_identical_to_u16() {
        // repr(transparent) makes this an assertion about the ABI, not just
        // about today's field count.
        assert_eq!(size_of::<MaterialId>(), size_of::<u16>());
        assert_eq!(align_of::<MaterialId>(), align_of::<u16>());
    }

    #[test]
    fn key_parses_and_splits() {
        let key = NamespacedKey::parse("mymod:copper_ore").unwrap();
        assert_eq!(key.namespace(), "mymod");
        assert_eq!(key.path(), "copper_ore");
        assert_eq!(key.as_str(), "mymod:copper_ore");
        assert_eq!(key.to_string(), "mymod:copper_ore");
    }

    #[test]
    fn key_allows_directories_in_path() {
        let key = NamespacedKey::new("mymod", "ores/deep/copper_ore").unwrap();
        assert_eq!(key.namespace(), "mymod");
        assert_eq!(key.path(), "ores/deep/copper_ore");
    }

    #[test]
    fn key_rejects_invalid_input() {
        assert_eq!(
            NamespacedKey::parse("copper_ore"),
            Err(KeyError::MissingSeparator)
        );
        assert_eq!(
            NamespacedKey::parse(":stone"),
            Err(KeyError::EmptyNamespace)
        );
        assert_eq!(NamespacedKey::parse("mymod:"), Err(KeyError::EmptyPath));
        assert_eq!(
            NamespacedKey::parse("MyMod:stone"),
            Err(KeyError::InvalidNamespaceChar('M'))
        );
        assert_eq!(
            NamespacedKey::parse("mymod:Stone Block"),
            Err(KeyError::InvalidPathChar('S'))
        );
    }

    #[test]
    fn builtin_shorthand_produces_canonical_key() {
        let key = NamespacedKey::builtin("stone");
        assert_eq!(key.as_str(), "voxel:stone");
        assert_eq!(key.namespace(), "voxel");
    }

    /// `builtin()` used a `debug_assert`, so an invalid path was silently
    /// accepted in release builds — minting a key `parse()` would reject and
    /// breaking the "every key is validated" invariant for all downstream
    /// HashMap lookups. This pins the check in both configurations.
    #[test]
    fn builtin_rejects_invalid_paths_even_in_release() {
        // A path with uppercase, spaces, or an empty path must panic, not
        // produce a key that fails to round-trip through `parse`.
        for bad in ["Stone Block", "STONE", ""] {
            let result = std::panic::catch_unwind(|| NamespacedKey::builtin(bad));
            assert!(result.is_err(), "expected panic for {bad:?}");
        }

        // And a valid path round-trips: `builtin(x).as_str()` parses back.
        let ok = NamespacedKey::builtin("block/stone");
        assert_eq!(NamespacedKey::parse(ok.as_str()), Ok(ok.clone()));
    }

    /// Every built-in `BlockKind` path must satisfy the same rule the panic
    /// enforces, so no kind trips the assertion at registration time.
    #[test]
    fn all_block_kind_paths_are_valid() {
        for kind in crate::kind::BlockKind::ALL {
            let key = kind.key();
            assert_eq!(key.namespace(), "voxel");
            assert_eq!(NamespacedKey::parse(key.as_str()), Ok(key));
        }
    }

    /// Simulates a future fast-path constructor (interning, deserialization)
    /// that produces a valid canonical string but a bogus `sep` cache.
    ///
    /// This is the regression guard for the hand-written `Hash`/`Eq`/`Ord`:
    /// if these impls ever consult `sep` again, the assertions below fail.
    #[test]
    fn identity_ignores_the_sep_cache() {
        let correct = NamespacedKey::new("mymod", "copper_ore").unwrap();
        let bogus_sep = NamespacedKey {
            raw: correct.raw.clone(),
            sep: 0, // wrong: "mymod" is 5 bytes
        };

        // Same canonical string == same key, regardless of the cache.
        assert_eq!(correct, bogus_sep);
        assert_eq!(correct.as_str(), bogus_sep.as_str());
        assert!(!(correct < bogus_sep) && !(bogus_sep < correct));

        // ...and therefore they must land in the same HashMap bucket.
        let mut map = std::collections::HashMap::new();
        map.insert(correct.clone(), 1u32);
        assert_eq!(map.get(&bogus_sep), Some(&1));
        map.insert(bogus_sep, 2);
        assert_eq!(map.len(), 1, "equal keys must not create two entries");
    }

    #[test]
    fn distinct_keys_are_still_distinct() {
        // Guard the flip side: excluding `sep` must not collapse genuinely
        // different keys.
        let a = NamespacedKey::new("mod", "ore").unwrap();
        let b = NamespacedKey::new("mo", "dore").unwrap();
        assert_ne!(a, b);
        // Same first 3 bytes, then ':' (0x3A) < 'd' (0x64), so `b` sorts first.
        assert!(b < a);

        let mut set = std::collections::HashSet::new();
        assert!(set.insert(a));
        assert!(set.insert(b));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn equal_keys_hash_equal_across_allocations() {
        // `Arc<str>` must compare/hash by contents, not pointer identity.
        let a = NamespacedKey::new("mymod", "copper_ore").unwrap();
        let b = NamespacedKey::parse("mymod:copper_ore").unwrap();
        assert!(!std::sync::Arc::ptr_eq(&a.raw, &b.raw));
        assert_eq!(a, b);

        use std::hash::{Hash, Hasher};
        let hash_of = |k: &NamespacedKey| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            k.hash(&mut h);
            h.finish()
        };
        assert_eq!(hash_of(&a), hash_of(&b));
    }
}
