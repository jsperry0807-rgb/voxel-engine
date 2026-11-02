//! M2-20: probe LOD rings and the content-event transition.
//!
//! ADR-005 §6 fixes ring membership as the horizontal distance from the camera
//! to the chunk AABB (near ≤ 128 m, mid ≤ 256 m, far beyond) and makes a ring
//! change a content event: the chunk reallocates and fully rebakes at the new
//! spacing, then flips. Targets that change before the rebuild starts coalesce,
//! so a crossing rebuilds once.
//!
//! The density is 512 / 64 / 0 probes at the pre-registered 128³ edge (and
//! 64 / 8 / 0 at 64³), one near probe per block and one mid probe per 2³
//! blocks.

use voxel_core::constants::{BLOCK_DIM, BLOCKS_PER_CHUNK, CHUNK_DIM, TARGET_CHUNK_DIM, VOXEL_SIZE};
use voxel_core::content_constants::{PROBE_MID_RADIUS_M, PROBE_NEAR_RADIUS_M};
use voxel_mesh::probe::{
    ProbeGrid, ProbeLod, ProbeLodState, SampleSource, bake, chunk_distance_m, chunk_probe_lod,
};
use voxel_storage::ChunkPos;

/// World edge of one chunk in metres: 4 m at 64³, 8 m at 128³.
fn chunk_extent_m() -> f64 {
    CHUNK_DIM as f64 * f64::from(VOXEL_SIZE)
}

/// A fully open volume at `lod` for the compiled chunk edge.
fn open_volume(lod: ProbeLod) -> voxel_mesh::probe::ProbeVolume {
    bake(
        ChunkPos::new(0, 0, 0),
        ProbeGrid::for_chunk(lod),
        |_, _, _| false,
    )
}

#[test]
fn probe_lod_rings_follow_the_horizontal_chunk_distance() {
    let extent = chunk_extent_m();
    let chunk = ChunkPos::new(0, 0, 0);

    // Inside the chunk footprint at any height is the near ring: height does
    // not participate in ring membership.
    let inside = [extent / 2.0, 10_000.0, extent / 2.0];
    assert_eq!(chunk_distance_m(chunk, inside), 0.0);
    assert_eq!(chunk_probe_lod(chunk, inside), ProbeLod::Near);

    // The radii are inclusive.
    let at_near = [extent + f64::from(PROBE_NEAR_RADIUS_M), 0.0, extent / 2.0];
    assert!((chunk_distance_m(chunk, at_near) - f64::from(PROBE_NEAR_RADIUS_M)).abs() < 1e-9);
    assert_eq!(chunk_probe_lod(chunk, at_near), ProbeLod::Near);
    let past_near = [at_near[0] + 0.001, 0.0, extent / 2.0];
    assert_eq!(chunk_probe_lod(chunk, past_near), ProbeLod::Mid);

    let at_mid = [extent + f64::from(PROBE_MID_RADIUS_M), 0.0, extent / 2.0];
    assert_eq!(chunk_probe_lod(chunk, at_mid), ProbeLod::Mid);
    assert_eq!(
        chunk_probe_lod(chunk, [at_mid[0] + 0.001, 0.0, extent / 2.0]),
        ProbeLod::Far
    );

    // Diagonal distance is Euclidean, not Chebyshev: both axes add.
    let diagonal = [extent + 128.0, 0.0, extent + 128.0];
    let expected = (128.0f64 * 128.0 * 2.0).sqrt();
    assert!((chunk_distance_m(chunk, diagonal) - expected).abs() < 1e-9);
    assert_eq!(chunk_probe_lod(chunk, diagonal), ProbeLod::Mid);

    // A camera west or south of the AABB measures from the min face.
    assert_eq!(chunk_distance_m(chunk, [-10.0, 0.0, extent / 2.0]), 10.0);
    assert_eq!(chunk_distance_m(chunk, [extent / 2.0, 0.0, -10.0]), 10.0);

    // Negative chunk coordinates use the same AABB rule.
    let west = ChunkPos::new(-1, 0, 0);
    assert_eq!(chunk_distance_m(west, [0.0, 0.0, 0.0]), 0.0);
    assert_eq!(
        chunk_probe_lod(west, [-extent / 2.0, 0.0, extent / 2.0]),
        ProbeLod::Near
    );

    // A non-finite camera has no ring: the far ring carries no probes.
    assert_eq!(chunk_distance_m(chunk, [f64::NAN, 0.0, 0.0]), f64::INFINITY);
    assert_eq!(
        chunk_probe_lod(chunk, [0.0, 0.0, f64::INFINITY]),
        ProbeLod::Far
    );
}

#[test]
fn probe_lod_counts_match_the_ring_density() {
    // 512 / 64 / 0 per chunk at the pre-registered 128³ edge.
    let target_edge_blocks = TARGET_CHUNK_DIM / BLOCK_DIM;
    assert_eq!(ProbeGrid::near(target_edge_blocks).len(), 512);
    assert_eq!(ProbeGrid::mid(target_edge_blocks).len(), 64);
    assert_eq!(ProbeGrid::far(target_edge_blocks).len(), 0);

    // The compiled edge follows the same rule: one per block, one eighth mid.
    let near = BLOCKS_PER_CHUNK;
    for (lod, expected) in [
        (ProbeLod::Near, near),
        (ProbeLod::Mid, near / 8),
        (ProbeLod::Far, 0),
    ] {
        let grid = ProbeGrid::for_chunk(lod);
        assert_eq!(grid.len(), expected, "{lod:?} grid");
        let volume = open_volume(lod);
        assert_eq!(volume.len(), expected, "{lod:?} volume");
        assert_eq!(volume.grid().lod(), lod);

        let sample = volume.sample([1.0, 1.0, 1.0]);
        match lod {
            ProbeLod::Far => assert_eq!(sample.source, SampleSource::Zero),
            ProbeLod::Near | ProbeLod::Mid => {
                assert_eq!(sample.source, SampleSource::Blend, "{lod:?}");
            }
        }
    }
}

#[test]
fn probe_lod_transition_coalesces_before_the_rebuild_starts() {
    let mut state = ProbeLodState::new(open_volume(ProbeLod::Near));

    // Cross both radii outward and come back to the bound ring before starting:
    // there is nothing to rebuild.
    state.request(ProbeLod::Mid);
    state.request(ProbeLod::Far);
    state.request(ProbeLod::Near);
    assert!(!state.is_pending());
    assert_eq!(state.begin(), None);
    assert_eq!(state.rebuilds(), 0);

    // Several crossings before the build starts produce one rebuild, at the
    // latest target only.
    state.request(ProbeLod::Mid);
    state.request(ProbeLod::Near);
    state.request(ProbeLod::Far);
    assert_eq!(state.target(), ProbeLod::Far);
    assert_eq!(state.begin(), Some(ProbeLod::Far));
    assert_eq!(state.building(), Some(ProbeLod::Far));
    assert_eq!(state.begin(), None, "the in-flight build is not restarted");
    assert_eq!(state.rebuilds(), 0);
    assert_eq!(state.current(), ProbeLod::Near, "the old grid stays bound");

    state.commit(open_volume(ProbeLod::Far));
    assert_eq!(state.current(), ProbeLod::Far);
    assert_eq!(
        state.rebuilds(),
        1,
        "three crossings coalesced into one build"
    );
    assert!(!state.is_pending());
    assert_eq!(state.begin(), None);
}

#[test]
fn probe_lod_transition_rebuilds_once_per_ring_crossing() {
    let mut state = ProbeLodState::new(open_volume(ProbeLod::Near));
    assert_eq!(state.rebuilds(), 0);

    for (step, target) in [ProbeLod::Mid, ProbeLod::Far, ProbeLod::Near]
        .into_iter()
        .enumerate()
    {
        state.request(target);
        assert_eq!(state.begin(), Some(target));
        state.commit(open_volume(target));
        assert_eq!(state.current(), target);
        assert_eq!(state.rebuilds(), step + 1);
        assert!(!state.is_pending());
    }

    // Repeating the current ring queues nothing.
    state.request(ProbeLod::Near);
    assert_eq!(state.begin(), None);
    assert_eq!(state.rebuilds(), 3);
}

#[test]
fn probe_lod_transition_queues_a_change_during_a_build() {
    let mut state = ProbeLodState::new(open_volume(ProbeLod::Near));

    state.request(ProbeLod::Mid);
    assert_eq!(state.begin(), Some(ProbeLod::Mid));

    // The camera crosses the mid radius while the mid volume is baking.
    state.request(ProbeLod::Far);
    assert_eq!(state.begin(), None, "the in-flight build is not restarted");
    state.commit(open_volume(ProbeLod::Mid));
    assert_eq!(state.current(), ProbeLod::Mid);
    assert_eq!(state.target(), ProbeLod::Far);
    assert!(state.is_pending());

    assert_eq!(state.begin(), Some(ProbeLod::Far));
    state.commit(open_volume(ProbeLod::Far));
    assert_eq!(state.rebuilds(), 2);
    assert!(!state.is_pending());
}

#[test]
fn probe_lod_transition_keeps_the_old_grid_bound_until_commit() {
    let mut state = ProbeLodState::new(open_volume(ProbeLod::Near));
    let near_len = state.volume().len();

    state.request(ProbeLod::Mid);
    assert_eq!(state.begin(), Some(ProbeLod::Mid));
    // The previous volume stays bound and samplable until the flip.
    assert_eq!(state.current(), ProbeLod::Near);
    assert_eq!(state.volume().grid().lod(), ProbeLod::Near);
    assert_eq!(state.volume().len(), near_len);
    assert_eq!(
        state.volume().sample([1.0, 1.0, 1.0]).source,
        SampleSource::Blend
    );

    state.commit(open_volume(ProbeLod::Mid));
    assert_eq!(state.volume().grid().lod(), ProbeLod::Mid);
    assert_eq!(state.volume().len(), near_len / 8);

    state.request(ProbeLod::Far);
    assert_eq!(state.begin(), Some(ProbeLod::Far));
    state.commit(open_volume(ProbeLod::Far));
    assert!(state.volume().is_empty());
    assert_eq!(
        state.volume().sample([1.0, 1.0, 1.0]).source,
        SampleSource::Zero
    );
}

#[test]
fn probe_lod_transition_aborts_an_in_flight_build() {
    let mut state = ProbeLodState::new(open_volume(ProbeLod::Near));

    state.request(ProbeLod::Mid);
    assert_eq!(state.begin(), Some(ProbeLod::Mid));
    state.abort();

    assert_eq!(state.building(), None);
    assert_eq!(
        state.current(),
        ProbeLod::Near,
        "the bound grid is untouched"
    );
    assert!(state.is_pending(), "the target is still queued");
    assert_eq!(
        state.begin(),
        Some(ProbeLod::Mid),
        "the build can be retried"
    );
    state.commit(open_volume(ProbeLod::Mid));
    assert_eq!(state.rebuilds(), 1);
}

#[test]
#[should_panic(expected = "commit without begin")]
fn probe_lod_commit_requires_a_started_build() {
    let mut state = ProbeLodState::new(open_volume(ProbeLod::Near));
    state.commit(open_volume(ProbeLod::Mid));
}

#[test]
#[should_panic(expected = "chunk edge")]
fn probe_lod_commit_rejects_another_edge() {
    let mut state = ProbeLodState::new(open_volume(ProbeLod::Near));
    state.request(ProbeLod::Mid);
    assert!(state.begin().is_some());

    let wrong_edge = bake(ChunkPos::new(0, 0, 0), ProbeGrid::mid(2), |_, _, _| false);
    state.commit(wrong_edge);
}

#[test]
fn probe_lod_request_at_reads_the_camera_ring() {
    let mut state = ProbeLodState::new(open_volume(ProbeLod::Near));
    let chunk = ChunkPos::new(0, 0, 0);
    let extent = chunk_extent_m();

    let ring = state.request_at(chunk, [extent + 200.0, 0.0, extent / 2.0]);
    assert_eq!(ring, ProbeLod::Mid);
    assert_eq!(state.target(), ProbeLod::Mid);
    assert!(state.is_pending());

    // Recrossing back to the bound ring before the build starts clears it.
    let ring = state.request_at(chunk, [extent / 2.0, 0.0, extent / 2.0]);
    assert_eq!(ring, ProbeLod::Near);
    assert!(!state.is_pending());
    assert_eq!(state.begin(), None);
}
