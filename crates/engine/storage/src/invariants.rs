//! Structural invariant checker for the banded chunk layout (M1-14).
//!
//! [`ChunkDag::check`] walks a chunk and validates the properties the rest of
//! the storage layer relies on:
//!
//! - **Band bounds.** Every live [`NodeRef`] names a slot inside its own band;
//!   a child run, a leaf run and the root stay inside their arenas. No live run
//!   may overlap another live run in the same band.
//! - **Level match.** A node word's encoded level equals the band it lives in,
//!   and child `c` of a level-`l` node is a level-`l - 1` word. Level 0 is
//!   never populated.
//! - **Alignment.** A full (`mask == 0xFF`) internal run starts on a 64 B
//!   boundary; every band's slot zero is 64 B aligned. Partial runs are packed
//!   and therefore only need the arena's own alignment.
//! - **Dedup bijection.** Every live run base denotes exactly one run content:
//!   two parents may share a base only when their mask and ordered children (or,
//!   for a leaf run, their ordered materials) agree. The checker also reports
//!   duplicate content at distinct bases; COW deliberately appends an unshared
//!   run rather than rebuilding an interner, so duplicates are a metric, not an
//!   error.
//! - **Parent reachability.** Every node reached from the root resolves, every
//!   present child is inside its band, and every reachable node is accounted
//!   for by a parent run or by the root itself.
//! - **Refcounts.** The checker derives a reference count for every reachable
//!   node and every live run. A run's nodes are referenced at least as many
//!   times as the run itself, and no reachable node has a zero count. The live
//!   root and in-flight-frame counters are reported and must be internally
//!   consistent.
//!
//! The checker never mutates the chunk. It returns the first violation as an
//! [`InvariantError`]; [`ChunkDag::assert_invariants`] is the panicking form
//! for debug builds and tests.
//!
//! # Example
//!
//! ```
//! use voxel_storage::{CHUNK_VOLUME, ChunkDag, local_index};
//!
//! let mut dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
//! dag.apply_edits([(local_index(4, 5, 6), 7)]);
//!
//! let report = dag.check_report().expect("a COW edit preserves the invariants");
//! assert!(report.nodes > 0);
//! assert_eq!(report.live_roots, 0);
//! assert!(dag.check().is_ok());
//! ```

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use voxel_core::constants::ROOT_LEVEL;
use voxel_core::node_word::{NODE_FLAGS_BITS, NODE_FLAGS_SHIFT};

use crate::bands::{BAND_ALIGN_BYTES, BAND_COUNT, Bands};
use crate::chunk::ChunkDag;
use crate::leaf::{AIR, LEAF_RUN_MATERIALS};
use crate::node_word::{Node, NodeRef};
use crate::run::{CHILD_SLOTS, FULL_MASK, FULL_RUN_NODES};

/// Mask of the reserved flag bits in a node word.
const RESERVED_FLAGS_MASK: u64 = ((1u64 << NODE_FLAGS_BITS) - 1) << NODE_FLAGS_SHIFT;

/// The category of an invariant violation.
///
/// The variants are stable enough for tests to match on them; the
/// [`InvariantError`] detail carries the offending level, slot and values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InvariantKind {
    /// A reference, run or root lies outside its arena.
    BandBounds,
    /// A node word's encoded level does not name the band it is stored in.
    LevelMismatch,
    /// A node word has a reserved flag bit set.
    ReservedFlags,
    /// A full run does not start on a 64 B boundary.
    Alignment,
    /// One live base denotes two different run contents.
    DedupBijection,
    /// A child or run does not resolve from the root.
    ParentReachability,
    /// A derived reference count is impossible.
    Refcount,
    /// A leaf run leaves the leaf arena or stores air.
    LeafBounds,
    /// The root is absent when it should exist, or names the wrong band/slot.
    Root,
    /// Level 0 carries nodes even though the banded build never uses it.
    UnexpectedLevelZero,
}

impl fmt::Display for InvariantKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::BandBounds => "band bounds",
            Self::LevelMismatch => "level match",
            Self::ReservedFlags => "reserved flags",
            Self::Alignment => "alignment",
            Self::DedupBijection => "dedup bijection",
            Self::ParentReachability => "parent reachability",
            Self::Refcount => "refcount",
            Self::LeafBounds => "leaf bounds",
            Self::Root => "root",
            Self::UnexpectedLevelZero => "unexpected level 0",
        };
        f.write_str(name)
    }
}

/// One invariant violation, with enough context to reproduce it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvariantError {
    kind: InvariantKind,
    detail: String,
}

impl InvariantError {
    fn new(kind: InvariantKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    /// The violated invariant.
    pub const fn kind(&self) -> InvariantKind {
        self.kind
    }

    /// A human-readable description of the offending node or run.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for InvariantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} invariant violated: {}", self.kind, self.detail)
    }
}

impl std::error::Error for InvariantError {}

/// Counts gathered by a successful [`ChunkDag::check_report`].
///
/// The counts are derived from the current root only. Retired runs and dead
/// band slots are allowed to exist until compaction; they are not included.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// Reachable internal nodes, including the root.
    pub nodes: u64,
    /// Distinct live internal runs (one per parent run base).
    pub internal_runs: u64,
    /// Distinct live leaf runs.
    pub leaf_runs: u64,
    /// Live runs referenced by more than one parent.
    pub shared_runs: u64,
    /// Live runs whose content also exists at another base.
    ///
    /// A COW edit appends a new run instead of rebuilding an interner, so this
    /// is expected to be non-zero after edits. It is reported for the layout
    /// cost and dedup-ratio tools (M1-18).
    pub duplicate_runs: u64,
    /// Largest number of parents that reference one run.
    pub max_run_refcount: u64,
    /// Live-root guards currently held by the chunk.
    pub live_roots: u32,
    /// In-flight-frame guards currently held by the chunk.
    pub in_flight_frames: u32,
}

/// One live run, identified by the arena it occupies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum RunSlot {
    Leaf(u64),
    Internal { level: u32, base: u64 },
}

impl RunSlot {
    fn level(self) -> u32 {
        match self {
            Self::Leaf(_) => 0,
            Self::Internal { level, .. } => level,
        }
    }

    fn base(self) -> u64 {
        match self {
            Self::Leaf(base) | Self::Internal { base, .. } => base,
        }
    }
}

/// The content a live base must denote. Two parents may share a base only when
/// their content compares equal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RunContent {
    Leaf { materials: Vec<u16> },
    Internal { mask: u8, children: Vec<Node> },
}

impl RunContent {
    fn slots(&self) -> u64 {
        match self {
            Self::Leaf { materials } => materials.len() as u64,
            Self::Internal { mask, .. } => u64::from(mask.count_ones()),
        }
    }
}

/// Derived reference counts for one level band.
struct NodeRefs {
    counts: [Vec<u64>; BAND_COUNT],
}

impl NodeRefs {
    fn new() -> Self {
        Self {
            counts: core::array::from_fn(|_| Vec::new()),
        }
    }

    fn add(&mut self, node_ref: NodeRef, amount: u64) {
        let level = node_ref.level() as usize;
        let index = usize::try_from(node_ref.index()).expect("node index does not fit usize");
        let counts = &mut self.counts[level];
        if counts.len() <= index {
            counts.resize(index + 1, 0);
        }
        counts[index] = counts[index]
            .checked_add(amount)
            .expect("node reference count overflows u64");
    }

    fn get(&self, node_ref: NodeRef) -> u64 {
        let level = node_ref.level() as usize;
        let index = usize::try_from(node_ref.index()).expect("node index does not fit usize");
        self.counts[level].get(index).copied().unwrap_or(0)
    }
}

impl ChunkDag {
    /// Checks every structural invariant of this chunk.
    ///
    /// This is the M1-14 checker. It is read-only, allocates only small
    /// bookkeeping maps, and is intended for tests and debug builds. Use
    /// [`Self::check_report`] when the derived counts are useful.
    pub fn check(&self) -> Result<(), InvariantError> {
        self.check_report().map(|_| ())
    }

    /// Checks every structural invariant and returns the derived counts.
    ///
    /// See [`Self::check`] for the list of validated properties. The report is
    /// produced only after the whole chunk passes; a violation returns
    /// [`InvariantError`] instead.
    pub fn check_report(&self) -> Result<CheckReport, InvariantError> {
        let mut report = check_bands(self.bands(), self.root())?;
        report.live_roots = self.live_root_count();
        report.in_flight_frames = self.in_flight_frame_count();
        Ok(report)
    }

    /// Panics with the first invariant violation.
    ///
    /// This is the debug-build form: a valid chunk returns normally.
    ///
    /// # Panics
    ///
    /// Panics if [`Self::check`] returns an error.
    pub fn assert_invariants(&self) {
        if let Err(error) = self.check() {
            panic!("chunk invariant violated: {error}");
        }
    }

    /// True when every invariant checked by [`Self::check`] holds.
    pub fn is_consistent(&self) -> bool {
        self.check().is_ok()
    }
}

impl Bands {
    /// Checks the arena-only invariants: band bounds, level words, reserved
    /// flags, 64 B alignment and the absence of stored air.
    ///
    /// A band arena has no root, so this cannot check parent reachability or
    /// run dedup. Use [`ChunkDag::check`] (or [`check_bands`]) for those.
    pub fn check(&self) -> Result<(), InvariantError> {
        check_band_arena(self)?;
        check_leaf_arena(self)
    }

    /// True when every arena-only invariant holds.
    pub fn is_consistent(&self) -> bool {
        self.check().is_ok()
    }
}

/// Checks a [`Bands`] arena plus an optional root without going through a
/// [`ChunkDag`].
///
/// This is the shared implementation behind [`ChunkDag::check`]. It is useful
/// to serializers and to tests that build a band arena directly.
pub fn check_bands(bands: &Bands, root: Option<NodeRef>) -> Result<CheckReport, InvariantError> {
    check_band_arena(bands)?;
    check_leaf_arena(bands)?;

    let mut report = CheckReport::default();
    let Some(root) = root else {
        // A chunk may keep dead bands after carving every voxel out of it, but
        // an absent root still means there is no live graph to walk.
        return Ok(report);
    };

    if root.level() != ROOT_LEVEL {
        return Err(InvariantError::new(
            InvariantKind::Root,
            format!("root {:?} must live in the level {ROOT_LEVEL} band", root),
        ));
    }
    if root.index() >= bands.len(ROOT_LEVEL) {
        return Err(InvariantError::new(
            InvariantKind::Root,
            format!(
                "root slot {} is past the level {ROOT_LEVEL} band tail {}",
                root.index(),
                bands.len(ROOT_LEVEL)
            ),
        ));
    }

    let mut visited: [Vec<bool>; BAND_COUNT] = core::array::from_fn(|_| Vec::new());
    let mut refs = NodeRefs::new();
    // The published root is one external reference to the top node.
    refs.add(root, 1);

    let mut base_content: BTreeMap<RunSlot, RunContent> = BTreeMap::new();
    let mut content_base: HashMap<RunContent, RunSlot> = HashMap::new();
    let mut run_refcounts: HashMap<RunSlot, u64> = HashMap::new();
    let mut stack = vec![root];

    while let Some(node_ref) = stack.pop() {
        let level = node_ref.level();
        let index = usize::try_from(node_ref.index()).map_err(|_| {
            InvariantError::new(
                InvariantKind::BandBounds,
                format!("node index {} does not fit usize", node_ref.index()),
            )
        })?;

        let marks = &mut visited[level as usize];
        if marks.len() <= index {
            marks.resize(index + 1, false);
        }
        if marks[index] {
            continue;
        }
        marks[index] = true;

        if node_ref.index() >= bands.len(level) {
            return Err(InvariantError::new(
                InvariantKind::BandBounds,
                format!(
                    "live node {:?} is past the level {level} band tail {}",
                    node_ref,
                    bands.len(level)
                ),
            ));
        }

        let node = bands.get(node_ref).ok_or_else(|| {
            InvariantError::new(
                InvariantKind::ParentReachability,
                format!("live node {node_ref:?} does not resolve"),
            )
        })?;
        check_node_word(node_ref, node)?;
        if node.level() == 0 {
            return Err(InvariantError::new(
                InvariantKind::UnexpectedLevelZero,
                format!("reachable node {node_ref:?} claims level 0"),
            ));
        }
        if node.mask() == 0 {
            return Err(InvariantError::new(
                InvariantKind::ParentReachability,
                format!("reachable node {node_ref:?} has an empty mask"),
            ));
        }

        report.nodes += 1;

        if level == 1 {
            check_leaf_parent(
                bands,
                node_ref,
                node,
                &mut base_content,
                &mut content_base,
                &mut run_refcounts,
                &mut report,
            )?;
            continue;
        }

        let child_level = level - 1;
        let child_band_len = bands.len(child_level);
        let run_slots = u64::from(node.mask().count_ones());
        let base = node.base();
        let end = base.checked_add(run_slots).ok_or_else(|| {
            InvariantError::new(
                InvariantKind::BandBounds,
                format!("run at level {child_level} base {base} overflows the slot space"),
            )
        })?;
        if end > child_band_len {
            return Err(InvariantError::new(
                InvariantKind::BandBounds,
                format!(
                    "run at level {child_level} base {base}..{end} is past the band tail \
                     {child_band_len}"
                ),
            ));
        }

        if node.mask() == FULL_MASK {
            let byte_offset = base
                .checked_mul(core::mem::size_of::<Node>() as u64)
                .ok_or_else(|| {
                    InvariantError::new(
                        InvariantKind::Alignment,
                        format!("full run at level {child_level} base {base} overflows bytes"),
                    )
                })?;
            if !byte_offset.is_multiple_of(BAND_ALIGN_BYTES as u64) {
                return Err(InvariantError::new(
                    InvariantKind::Alignment,
                    format!("full run at level {child_level} base {base} is not 64 B aligned"),
                ));
            }
            let band_start = bands.band(child_level).as_ptr() as usize;
            if !(band_start + byte_offset as usize).is_multiple_of(BAND_ALIGN_BYTES) {
                return Err(InvariantError::new(
                    InvariantKind::Alignment,
                    format!(
                        "full run at level {child_level} base {base} does not land on a \
                         cache line in memory"
                    ),
                ));
            }
        }

        let mut children = Vec::with_capacity(run_slots as usize);
        for c in 0..CHILD_SLOTS {
            let Some(slot) = node.child_slot(c) else {
                continue;
            };
            let child_index = base + u64::from(slot);
            let child_ref = NodeRef::new(child_level, child_index);
            if child_index >= child_band_len {
                return Err(InvariantError::new(
                    InvariantKind::BandBounds,
                    format!(
                        "child {c} of {node_ref:?} points at {child_ref:?}, past the band tail \
                         {child_band_len}"
                    ),
                ));
            }
            let child = bands.get(child_ref).ok_or_else(|| {
                InvariantError::new(
                    InvariantKind::ParentReachability,
                    format!("child {c} of {node_ref:?} is missing at {child_ref:?}"),
                )
            })?;
            check_node_word(child_ref, child)?;
            if child.level() != child_level {
                return Err(InvariantError::new(
                    InvariantKind::LevelMismatch,
                    format!(
                        "child {c} of {node_ref:?} is a level {} word in the level \
                         {child_level} band",
                        child.level()
                    ),
                ));
            }
            if child.mask() == 0 {
                return Err(InvariantError::new(
                    InvariantKind::ParentReachability,
                    format!("child {c} of {node_ref:?} is an empty node"),
                ));
            }
            children.push(child);
            refs.add(child_ref, 1);
            stack.push(child_ref);
        }

        let content = RunContent::Internal {
            mask: node.mask(),
            children,
        };
        record_run(
            RunSlot::Internal {
                level: child_level,
                base,
            },
            content,
            &mut base_content,
            &mut content_base,
            &mut run_refcounts,
            &mut report,
        )?;
    }

    // Every reachable node must have at least one reference (the root's
    // external reference or a parent run slot).
    for level in 0..=ROOT_LEVEL {
        for (index, &live) in visited[level as usize].iter().enumerate() {
            if !live {
                continue;
            }
            let node_ref = NodeRef::new(level, index as u64);
            if refs.get(node_ref) == 0 {
                return Err(InvariantError::new(
                    InvariantKind::Refcount,
                    format!("reachable node {node_ref:?} has a zero reference count"),
                ));
            }
        }
    }

    // A shared run must contribute its own reference count to every node in
    // the run; the total may be higher when a node is also in another run.
    for (&slot, &run_refs) in &run_refcounts {
        let Some(content) = base_content.get(&slot) else {
            return Err(InvariantError::new(
                InvariantKind::Refcount,
                format!("live run {slot:?} has no recorded content"),
            ));
        };
        match content {
            RunContent::Leaf { .. } => {
                // Leaf slots are materials, not internal nodes; the leaf arena
                // is checked by `check_leaf_parent` and `check_leaf_arena`.
            }
            RunContent::Internal { children, .. } => {
                let child_level = slot.level();
                for (offset, _) in children.iter().enumerate() {
                    let child_ref = NodeRef::new(child_level, slot.base() + offset as u64);
                    if refs.get(child_ref) < run_refs {
                        return Err(InvariantError::new(
                            InvariantKind::Refcount,
                            format!(
                                "node {child_ref:?} is referenced {} time(s) but its run \
                                 {slot:?} is referenced {run_refs} time(s)",
                                refs.get(child_ref)
                            ),
                        ));
                    }
                }
            }
        }
    }

    report.shared_runs = run_refcounts.values().filter(|&&refs| refs > 1).count() as u64;
    check_run_overlaps(bands, &base_content)?;
    Ok(report)
}

/// Validates the band arena itself: level words, reserved flags, alignment and
/// the unused level-0 band.
fn check_band_arena(bands: &Bands) -> Result<(), InvariantError> {
    if bands.len(0) != 0 {
        return Err(InvariantError::new(
            InvariantKind::UnexpectedLevelZero,
            format!("level 0 band holds {} node(s)", bands.len(0)),
        ));
    }

    for level in 0..=ROOT_LEVEL {
        let band = bands.band(level);
        if !band.is_empty() && !(band.as_ptr() as usize).is_multiple_of(BAND_ALIGN_BYTES) {
            return Err(InvariantError::new(
                InvariantKind::Alignment,
                format!("level {level} band slot zero is not 64 B aligned"),
            ));
        }
        for (index, &node) in band.iter().enumerate() {
            let node_ref = NodeRef::new(level, index as u64);
            check_node_word(node_ref, node)?;
            if node.mask() == FULL_MASK && level >= 2 {
                let run_level = level - 1;
                let byte_offset = node.base() * core::mem::size_of::<Node>() as u64;
                if !byte_offset.is_multiple_of(FULL_RUN_NODES * core::mem::size_of::<Node>() as u64)
                {
                    return Err(InvariantError::new(
                        InvariantKind::Alignment,
                        format!(
                            "node {node_ref:?} has a full run at level {run_level} base {} \
                             that is not 64 B aligned",
                            node.base()
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Validates that the leaf arena never stores air.
fn check_leaf_arena(bands: &Bands) -> Result<(), InvariantError> {
    for (index, &material) in bands.leaves().iter().enumerate() {
        if material == AIR {
            return Err(InvariantError::new(
                InvariantKind::LeafBounds,
                format!("leaf slot {index} stores air ({AIR})"),
            ));
        }
    }
    Ok(())
}

/// Validates one node word independent of its position.
fn check_node_word(node_ref: NodeRef, node: Node) -> Result<(), InvariantError> {
    if node.level() != node_ref.level() {
        return Err(InvariantError::new(
            InvariantKind::LevelMismatch,
            format!("node {node_ref:?} carries level {}", node.level()),
        ));
    }
    if node.encode() & RESERVED_FLAGS_MASK != 0 {
        return Err(InvariantError::new(
            InvariantKind::ReservedFlags,
            format!("node {node_ref:?} has reserved flag bits set"),
        ));
    }
    Ok(())
}

/// Records a level-1 parent's leaf run.
#[allow(clippy::too_many_arguments)]
fn check_leaf_parent(
    bands: &Bands,
    node_ref: NodeRef,
    node: Node,
    base_content: &mut BTreeMap<RunSlot, RunContent>,
    content_base: &mut HashMap<RunContent, RunSlot>,
    run_refcounts: &mut HashMap<RunSlot, u64>,
    report: &mut CheckReport,
) -> Result<(), InvariantError> {
    debug_assert_eq!(node.level(), 1);
    let count = u64::from(node.mask().count_ones());
    let base = node.base();
    let end = base.checked_add(count).ok_or_else(|| {
        InvariantError::new(
            InvariantKind::LeafBounds,
            format!("leaf run of {node_ref:?} overflows the slot space"),
        )
    })?;
    if end > bands.leaf_len() {
        return Err(InvariantError::new(
            InvariantKind::LeafBounds,
            format!(
                "leaf run of {node_ref:?} spans {base}..{end}, past the leaf arena tail {}",
                bands.leaf_len()
            ),
        ));
    }
    if count > LEAF_RUN_MATERIALS as u64 {
        return Err(InvariantError::new(
            InvariantKind::LeafBounds,
            format!("leaf run of {node_ref:?} holds {count} materials"),
        ));
    }

    let mut materials = Vec::with_capacity(count as usize);
    for c in 0..CHILD_SLOTS {
        let Some(slot) = node.child_slot(c) else {
            continue;
        };
        let leaf_index = base + u64::from(slot);
        let material = bands.leaf(leaf_index).ok_or_else(|| {
            InvariantError::new(
                InvariantKind::LeafBounds,
                format!(
                    "leaf child {c} of {node_ref:?} points at slot {leaf_index}, past the \
                     arena tail {}",
                    bands.leaf_len()
                ),
            )
        })?;
        if material == AIR {
            return Err(InvariantError::new(
                InvariantKind::LeafBounds,
                format!("leaf child {c} of {node_ref:?} stores air"),
            ));
        }
        materials.push(material);
    }

    record_run(
        RunSlot::Leaf(base),
        RunContent::Leaf { materials },
        base_content,
        content_base,
        run_refcounts,
        report,
    )
}

/// Updates the base/content bijection and run reference counts.
fn record_run(
    slot: RunSlot,
    content: RunContent,
    base_content: &mut BTreeMap<RunSlot, RunContent>,
    content_base: &mut HashMap<RunContent, RunSlot>,
    run_refcounts: &mut HashMap<RunSlot, u64>,
    report: &mut CheckReport,
) -> Result<(), InvariantError> {
    match base_content.get(&slot) {
        Some(existing) if *existing != content => {
            return Err(InvariantError::new(
                InvariantKind::DedupBijection,
                format!(
                    "run base {slot:?} denotes two different contents: {existing:?} and \
                     {content:?}"
                ),
            ));
        }
        Some(_) => {}
        None => {
            base_content.insert(slot, content.clone());
            match slot {
                RunSlot::Leaf(_) => report.leaf_runs += 1,
                RunSlot::Internal { .. } => report.internal_runs += 1,
            }
            if content_base.insert(content, slot).is_some() {
                report.duplicate_runs += 1;
            }
        }
    }

    let refs = run_refcounts.entry(slot).or_insert(0);
    *refs += 1;
    report.max_run_refcount = report.max_run_refcount.max(*refs);
    Ok(())
}

/// Rejects two distinct live runs whose slot ranges overlap in one arena.
fn check_run_overlaps(
    bands: &Bands,
    base_content: &BTreeMap<RunSlot, RunContent>,
) -> Result<(), InvariantError> {
    let mut leaf_runs: Vec<(u64, u64)> = Vec::new();
    let mut internal_runs: [Vec<(u64, u64)>; BAND_COUNT] = core::array::from_fn(|_| Vec::new());

    for (&slot, content) in base_content {
        let start = slot.base();
        let end = start + content.slots();
        match slot {
            RunSlot::Leaf(_) => leaf_runs.push((start, end)),
            RunSlot::Internal { level, .. } => {
                if level as usize >= BAND_COUNT {
                    return Err(InvariantError::new(
                        InvariantKind::BandBounds,
                        format!("run {slot:?} names a level outside the chunk"),
                    ));
                }
                internal_runs[level as usize].push((start, end));
            }
        }
    }

    check_ranges(&mut leaf_runs, "leaf arena", bands.leaf_len())?;
    for (level, runs) in internal_runs.iter_mut().enumerate() {
        check_ranges(
            runs,
            &format!("level {level} band"),
            bands.len(level as u32),
        )?;
    }
    Ok(())
}

/// Checks sorted ranges for overlap and for staying inside `limit`.
fn check_ranges(ranges: &mut [(u64, u64)], arena: &str, limit: u64) -> Result<(), InvariantError> {
    ranges.sort_unstable();
    let mut previous: Option<(u64, u64)> = None;
    for &(start, end) in ranges.iter() {
        if end > limit {
            return Err(InvariantError::new(
                InvariantKind::BandBounds,
                format!("run {start}..{end} in {arena} is past the arena tail {limit}"),
            ));
        }
        if let Some((previous_start, previous_end)) = previous
            && start < previous_end
        {
            return Err(InvariantError::new(
                InvariantKind::ParentReachability,
                format!(
                    "runs {previous_start}..{previous_end} and {start}..{end} overlap in {arena}"
                ),
            ));
        }
        previous = Some((start, end));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::HEADER_BYTES;
    use crate::{CHUNK_DIM, CHUNK_VOLUME, ChunkDag, local_index};

    /// Deterministic xorshift64, so a failure is reproducible.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn fixtures() -> Vec<(&'static str, Vec<u16>)> {
        let air = vec![0u16; CHUNK_VOLUME];
        let solid = vec![3u16; CHUNK_VOLUME];

        let mut slab = vec![0u16; CHUNK_VOLUME];
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    if y < 17 {
                        slab[local_index(x as u32, y as u32, z as u32) as usize] =
                            1 + (y % 3) as u16;
                    }
                }
            }
        }

        let mut checker = vec![0u16; CHUNK_VOLUME];
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    checker[local_index(x as u32, y as u32, z as u32) as usize] =
                        if (x + y + z) % 2 == 0 { 1 } else { 2 };
                }
            }
        }

        let mut noise = vec![0u16; CHUNK_VOLUME];
        let mut state = 0x5eed_1e55_c0de_1234u64;
        for voxel in &mut noise {
            let sample = rng(&mut state);
            if sample % 100 < 30 {
                *voxel = 1 + ((sample >> 8) % 7) as u16;
            }
        }

        vec![
            ("air", air),
            ("solid", solid),
            ("slab", slab),
            ("checker", checker),
            ("noise", noise),
        ]
    }

    fn blocky_base() -> Vec<u16> {
        let mut dense = vec![0u16; CHUNK_VOLUME];
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    let material = if y < 20 {
                        1
                    } else if (x / 8 + z / 8) % 3 == 0 && y < 30 {
                        2
                    } else {
                        AIR
                    };
                    dense[local_index(x as u32, y as u32, z as u32) as usize] = material;
                }
            }
        }
        dense
    }

    /// Byte offset of node `index` in `level` inside a v2 serialization.
    fn node_offset(lengths: &[u64; BAND_COUNT], level: u32, index: u64) -> usize {
        let before: u64 = lengths[..level as usize].iter().sum();
        HEADER_BYTES + ((before + index) * 8) as usize
    }

    fn serialized_lengths(bytes: &[u8]) -> [u64; BAND_COUNT] {
        core::array::from_fn(|level| {
            let start = 16 + level * 8;
            u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap())
        })
    }

    /// M1-14 acceptance: every fixture, including the empty and uniform
    /// collapses, passes the checker after a fresh BFS build.
    #[test]
    fn invariants_fresh_builds_pass_on_five_fixtures() {
        for (name, dense) in fixtures() {
            let dag = ChunkDag::from_dense(&dense);
            let report = dag
                .check_report()
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            if dense.iter().all(|&material| material == AIR) {
                assert_eq!(report.nodes, 0, "{name}");
                assert_eq!(report.internal_runs, 0, "{name}");
                assert_eq!(report.leaf_runs, 0, "{name}");
            } else {
                assert!(report.nodes > 0, "{name}");
                assert!(report.internal_runs > 0, "{name}");
                assert!(report.leaf_runs > 0, "{name}");
            }
            assert!(dag.check().is_ok(), "{name}");
            assert!(dag.is_consistent(), "{name}");
            dag.assert_invariants();
        }
    }

    /// A uniform chunk dedups to one run per level; the checker sees the
    /// sharing and reports it.
    #[test]
    fn invariants_uniform_report_counts_shared_runs() {
        let dag = ChunkDag::from_dense(&vec![7u16; CHUNK_VOLUME]);
        let report = dag.check_report().expect("uniform chunk is valid");

        assert_eq!(report.nodes, 1 + (ROOT_LEVEL as u64 - 1) * FULL_RUN_NODES);
        assert_eq!(report.internal_runs, ROOT_LEVEL as u64 - 1);
        assert_eq!(report.leaf_runs, 1);
        assert_eq!(report.duplicate_runs, 0);
        assert_eq!(report.max_run_refcount, FULL_RUN_NODES);
        assert!(report.shared_runs >= 1);
    }

    /// COW appends new runs without rebuilding an interner; the checker must
    /// accept the resulting duplicate (unshared) content and still validate
    /// reachability, bounds and alignment.
    #[test]
    fn invariants_cow_edits_pass_and_report_duplicates() {
        let mut dense = blocky_base();
        let mut dag = ChunkDag::from_dense(&dense);
        let mut state = 0x0c0a_5eed_1234_5678u64;

        for batch in 0..128 {
            let count = (rng(&mut state) % 9) as usize;
            let mut edits = Vec::with_capacity(count);
            for _ in 0..count {
                let index = (rng(&mut state) % CHUNK_VOLUME as u64) as u32;
                let material = (rng(&mut state) % 5) as u16;
                edits.push((index, material));
                dense[index as usize] = material;
            }
            dag.apply_edits(edits);
            dag.check()
                .unwrap_or_else(|error| panic!("batch {batch}: {error}"));
        }

        let mut rebuilt = vec![u16::MAX; CHUNK_VOLUME];
        dag.expand(&mut rebuilt);
        assert_eq!(rebuilt, dense);

        // The edit path is allowed to append content that already exists at
        // another base; the report counts those runs instead of rejecting them.
        let report = dag.check_report().expect("edited chunk is valid");
        assert!(report.duplicate_runs > 0);
    }

    /// The report includes the live-root and in-flight-frame reference counts
    /// used by the compaction guard.
    #[test]
    fn invariants_report_access_refcounts() {
        let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        assert_eq!(dag.check_report().unwrap().live_roots, 0);

        let root_guard = dag.retain_root();
        let frame_guard = dag.retain_frame();
        let report = dag.check_report().expect("guards do not break the graph");
        assert_eq!(report.live_roots, 1);
        assert_eq!(report.in_flight_frames, 1);

        drop(frame_guard);
        drop(root_guard);
        let report = dag.check_report().expect("released guards");
        assert_eq!(report.live_roots, 0);
        assert_eq!(report.in_flight_frames, 0);
    }

    /// A chunk carved back to air keeps dead bands; the checker must not treat
    /// unreachable slots as live graph.
    #[test]
    fn invariants_empty_chunk_with_dead_bands_passes() {
        let mut dag = ChunkDag::from_dense(&vec![1u16; CHUNK_VOLUME]);
        dag.apply_edits((0..CHUNK_VOLUME as u32).map(|index| (index, AIR)));

        assert!(dag.is_empty());
        assert!(dag.node_words() > 0, "dead bands remain until compaction");
        let report = dag.check_report().expect("dead bands are legal");
        assert_eq!(report.nodes, 0);
        assert_eq!(report.internal_runs, 0);
        assert_eq!(report.leaf_runs, 0);
    }

    /// Compaction renumbers every live run; the post-compaction graph must
    /// satisfy the same checker.
    #[test]
    fn invariants_compaction_preserves_the_invariants() {
        let mut dag = ChunkDag::from_dense(&blocky_base());
        let mut state = 0xfeed_face_cafe_beefu64;
        for _ in 0..64 {
            let index = (rng(&mut state) % CHUNK_VOLUME as u64) as u32;
            dag.apply_edits([(index, (rng(&mut state) % 5) as u16)]);
        }
        dag.check().expect("pre-compaction");

        let mut before = vec![0u16; CHUNK_VOLUME];
        dag.expand(&mut before);
        dag.compact();
        dag.check().expect("post-compaction");

        // A full renumber preserves the voxel content byte for byte.
        let mut after = vec![0u16; CHUNK_VOLUME];
        dag.expand(&mut after);
        assert_eq!(before, after);
    }

    /// Serialization round-trips a COW root and its stale bands; both the
    /// original and the reloaded chunk pass.
    #[test]
    fn invariants_serialization_roundtrip_passes() {
        let mut dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        dag.apply_edits([(local_index(3, 4, 5), 9), (local_index(17, 33, 41), 11)]);
        dag.check().expect("original");

        let bytes = dag.to_bytes();
        let reloaded = ChunkDag::from_bytes(&bytes).expect("valid bytes");
        reloaded.check().expect("reloaded");
        assert_eq!(reloaded, dag);
    }

    /// The checker must detect a node word whose encoded level does not name
    /// its band. The loader rejects such a word first; this exercises the
    /// checker's own level test directly.
    #[test]
    fn invariants_detect_wrong_level_words() {
        let wrong = Node::new(0b11, 2, 0);
        let error = check_node_word(NodeRef::new(3, 0), wrong).expect_err("wrong level");
        assert_eq!(error.kind(), InvariantKind::LevelMismatch);

        // A serialized word with a corrupted level is rejected before it can
        // reach the checker; detection is still total (None, never a panic).
        let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        let mut bytes = dag.to_bytes();
        let lengths = serialized_lengths(&bytes);
        let root_index = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let offset = node_offset(&lengths, ROOT_LEVEL, root_index);
        let word = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let bad = Node::decode(word ^ (1 << 8)); // flip a level bit
        bytes[offset..offset + 8].copy_from_slice(&bad.encode().to_le_bytes());
        assert!(ChunkDag::from_bytes(&bytes).is_none());
    }

    /// A root or parent base outside its child band is detected as a band
    /// bounds or reachability failure, not accepted or panicked on.
    #[test]
    fn invariants_detect_a_bad_base() {
        let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        let mut bytes = dag.to_bytes();
        let lengths = serialized_lengths(&bytes);
        let root_index = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let offset = node_offset(&lengths, ROOT_LEVEL, root_index);
        let word = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let node = Node::decode(word);

        // Keep the run 64 B aligned so the failure is specifically the band
        // bound, then point well past the level-5 band tail.
        let child_len = lengths[(ROOT_LEVEL - 1) as usize];
        let bad_base = (child_len + 8).next_multiple_of(FULL_RUN_NODES);
        let bad = Node::new(node.mask(), node.level(), bad_base);
        bytes[offset..offset + 8].copy_from_slice(&bad.encode().to_le_bytes());

        let corrupt = ChunkDag::from_bytes(&bytes).expect("the loader accepts a bad base");
        let error = corrupt.check().expect_err("bad base must be detected");
        assert!(
            matches!(
                error.kind(),
                InvariantKind::BandBounds | InvariantKind::ParentReachability
            ),
            "{error}"
        );
    }

    /// A full run at an unaligned base is detected even when the run is not
    /// reachable, because the band arena itself carries the alignment
    /// invariant.
    #[test]
    fn invariants_detect_an_unaligned_full_run() {
        let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        let mut bytes = dag.to_bytes();
        let lengths = serialized_lengths(&bytes);
        let root_index = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let offset = node_offset(&lengths, ROOT_LEVEL, root_index);
        let word = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let node = Node::decode(word);
        assert_eq!(node.mask(), FULL_MASK);

        let bad = Node::new(node.mask(), node.level(), node.base() + 1);
        bytes[offset..offset + 8].copy_from_slice(&bad.encode().to_le_bytes());

        let corrupt = ChunkDag::from_bytes(&bytes).expect("the loader accepts the base");
        let error = corrupt
            .check()
            .expect_err("unaligned full run must be detected");
        assert_eq!(error.kind(), InvariantKind::Alignment);
    }

    /// Reserved flag bits are ignored by traversal but forbidden by the layout;
    /// the checker rejects them.
    #[test]
    fn invariants_detect_reserved_flags() {
        let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        let mut bytes = dag.to_bytes();
        let lengths = serialized_lengths(&bytes);
        let root_index = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let offset = node_offset(&lengths, ROOT_LEVEL, root_index);
        let word = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        bytes[offset..offset + 8].copy_from_slice(&(word | (1 << NODE_FLAGS_SHIFT)).to_le_bytes());

        let corrupt = ChunkDag::from_bytes(&bytes).expect("flags are not validated by the loader");
        let error = corrupt
            .check()
            .expect_err("reserved flags must be detected");
        assert_eq!(error.kind(), InvariantKind::ReservedFlags);
    }

    /// The leaf arena never stores air; a zero slot is corruption.
    #[test]
    fn invariants_detect_air_in_the_leaf_arena() {
        let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        let mut bytes = dag.to_bytes();
        let lengths = serialized_lengths(&bytes);
        let nodes: u64 = lengths.iter().sum();
        let leaf_offset = HEADER_BYTES + (nodes * 8) as usize;
        bytes[leaf_offset..leaf_offset + 2].copy_from_slice(&AIR.to_le_bytes());

        let corrupt = ChunkDag::from_bytes(&bytes).expect("the loader accepts the leaf slot");
        let error = corrupt.check().expect_err("stored air must be detected");
        assert_eq!(error.kind(), InvariantKind::LeafBounds);
    }

    /// One base must never denote two different runs: the dedup relation is a
    /// function from base to content.
    #[test]
    fn invariants_detect_a_dedup_base_conflict() {
        let mut base_content = BTreeMap::new();
        let mut content_base = HashMap::new();
        let mut run_refcounts = HashMap::new();
        let mut report = CheckReport::default();
        let slot = RunSlot::Internal { level: 2, base: 0 };
        let first = RunContent::Internal {
            mask: 0b1,
            children: vec![Node::new(0b1, 2, 0)],
        };
        let second = RunContent::Internal {
            mask: 0b1,
            children: vec![Node::new(0b10, 2, 0)],
        };

        record_run(
            slot,
            first,
            &mut base_content,
            &mut content_base,
            &mut run_refcounts,
            &mut report,
        )
        .expect("first content");
        let error = record_run(
            slot,
            second,
            &mut base_content,
            &mut content_base,
            &mut run_refcounts,
            &mut report,
        )
        .expect_err("one base cannot hold two contents");
        assert_eq!(error.kind(), InvariantKind::DedupBijection);
    }
}
