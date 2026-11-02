//! M2-21: production probe updates are deterministic and match the M1-30 oracle.
//!
//! The gate measures the update path — select the probes an edit can reach,
//! rebake them, stage the bytes — so the pieces it times must be the production
//! ones (M2-18/M2-21 replace the M1 reference). This file pins their behaviour:
//!
//! 1. selection returns the same affected set and statistics as the oracle;
//! 2. an incremental rebake equals a fresh full bake after every edit;
//! 3. the update is deterministic across runs, threads and index order;
//! 4. the ray helper that picks the gate's edit targets matches the oracle.

use voxel_core::constants::BLOCK_DIM;
use voxel_mesh::probe::{
    PROBE_RAYS, ProbeGrid, ProbeVolume, bake, first_solid_on_ray, hemisphere_ray, probe_rotation,
    probe_seed, probes_reaching,
};
use voxel_storage::ChunkPos;
use voxel_storage::reference::probe as oracle;

const EDGE_BLOCKS: usize = 3;

/// A floor, a column and a floating block: probes on every face see something.
fn fixture_solid(x: u32, y: u32, z: u32) -> bool {
    let block = BLOCK_DIM as u32;
    let (bx, by, bz) = (x / block, y / block, z / block);

    if y < block {
        return true;
    }
    if bx == 1 && bz == 1 && by < 3 {
        return true;
    }
    bx == 2 && by == 2 && bz == 0
}

fn edge() -> usize {
    EDGE_BLOCKS * BLOCK_DIM
}

fn cell_index(edge: usize, x: u32, y: u32, z: u32) -> usize {
    x as usize + y as usize * edge + z as usize * edge * edge
}

/// Every solid voxel that is the first hit of a probe ray, one per probe.
fn first_hit_targets(
    chunk: ChunkPos,
    grid: ProbeGrid,
    solid: &[bool],
    edge: usize,
) -> Vec<[u32; 3]> {
    let mut targets = Vec::new();
    for probe in 0..grid.len() {
        let centre = grid.centre(probe);
        let centre_cell = [
            centre[0].floor() as u32,
            centre[1].floor() as u32,
            centre[2].floor() as u32,
        ];
        if solid[cell_index(edge, centre_cell[0], centre_cell[1], centre_cell[2])] {
            continue;
        }
        let rotation = probe_rotation(probe_seed(chunk, grid.lod(), probe as u32));
        for ray in 0..PROBE_RAYS {
            let direction = hemisphere_ray(ray, rotation);
            let hit = first_solid_on_ray(edge, centre, direction, |x, y, z| {
                solid[cell_index(edge, x, y, z)]
            });
            if let Some(target) = hit {
                targets.push(target);
                break;
            }
        }
    }
    targets
}

fn initial_fixture() -> Vec<bool> {
    let edge = edge();
    let mut solid = vec![false; edge * edge * edge];
    for z in 0..edge as u32 {
        for y in 0..edge as u32 {
            for x in 0..edge as u32 {
                solid[cell_index(edge, x, y, z)] = fixture_solid(x, y, z);
            }
        }
    }
    solid
}

fn full_bake(chunk: ChunkPos, grid: ProbeGrid, solid: &[bool]) -> ProbeVolume {
    let edge = edge();
    bake(chunk, grid, |x, y, z| solid[cell_index(edge, x, y, z)])
}

#[test]
fn probe_update_selection_matches_the_reference() {
    let chunk = ChunkPos::new(-3, 1, 7);
    let grid = ProbeGrid::near(EDGE_BLOCKS);
    let oracle_grid = oracle::ProbeGrid::near(EDGE_BLOCKS);
    let solid = initial_fixture();
    let edge = edge();
    let targets = first_hit_targets(chunk, grid, &solid, edge);
    assert!(targets.len() >= 4, "the fixture must expose targets");

    let mut affected = Vec::new();
    let mut expected = Vec::new();
    for target in targets.iter().take(16) {
        let reach = probes_reaching(
            chunk,
            grid,
            *target,
            |x, y, z| solid[cell_index(edge, x, y, z)],
            &mut affected,
        );
        let oracle_reach = oracle::probes_reaching(
            chunk,
            oracle_grid,
            *target,
            |x, y, z| solid[cell_index(edge, x, y, z)],
            &mut expected,
        );

        assert_eq!(affected, expected, "affected set for {target:?}");
        assert_eq!(reach.affected, oracle_reach.affected, "affected {target:?}");
        assert_eq!(
            reach.rays_tested, oracle_reach.rays_tested,
            "rays tested {target:?}"
        );
        assert_eq!(
            reach.rays_reaching, oracle_reach.rays_reaching,
            "rays reaching {target:?}"
        );
        assert!(reach.affected > 0, "{target:?} must be visible");
    }
}

#[test]
fn probe_update_matches_a_full_bake_across_edits() {
    let chunk = ChunkPos::new(-3, 1, 7);
    let grid = ProbeGrid::near(EDGE_BLOCKS);
    let edge = edge();
    let mut solid = initial_fixture();
    let targets = first_hit_targets(chunk, grid, &solid, edge);
    assert!(targets.len() >= 4, "the fixture must expose targets");

    let oracle_grid = oracle::ProbeGrid::near(EDGE_BLOCKS);
    let mut volume = full_bake(chunk, grid, &solid);
    let mut oracle_volume = oracle::bake(chunk, oracle_grid, |x, y, z| {
        solid[cell_index(edge, x, y, z)]
    });
    let mut affected = Vec::new();

    for target in targets.iter().take(12) {
        let index = cell_index(edge, target[0], target[1], target[2]);
        assert!(solid[index], "targets are first solids");

        // Selection sees the pre-edit state in both directions.
        for edit_solid in [false, true] {
            probes_reaching(
                chunk,
                grid,
                *target,
                |x, y, z| solid[cell_index(edge, x, y, z)],
                &mut affected,
            );
            solid[index] = edit_solid;

            let stats = volume.rebake(chunk, &affected, |x, y, z| solid[cell_index(edge, x, y, z)]);
            let oracle_stats =
                oracle_volume.rebake(chunk, &affected, |x, y, z| solid[cell_index(edge, x, y, z)]);
            assert_eq!(
                (stats.probes, stats.valid, stats.rays),
                (oracle_stats.probes, oracle_stats.valid, oracle_stats.rays),
                "stats after setting {target:?} to {edit_solid}"
            );
            assert_eq!(
                volume.sh_bytes(),
                oracle_volume.sh_bytes(),
                "oracle SH bytes after setting {target:?} to {edit_solid}"
            );
            assert_eq!(
                volume.validity_bytes(),
                oracle_volume.validity_bytes(),
                "oracle validity after setting {target:?} to {edit_solid}"
            );

            let full = full_bake(chunk, grid, &solid);
            assert_eq!(
                volume.sh_bytes(),
                full.sh_bytes(),
                "SH bytes after setting {target:?} to {edit_solid}"
            );
            assert_eq!(
                volume.validity_bytes(),
                full.validity_bytes(),
                "validity after setting {target:?} to {edit_solid}"
            );
        }
    }
}

#[test]
fn probe_update_flips_a_probe_whose_centre_is_edited() {
    let chunk = ChunkPos::new(4, 0, -2);
    let grid = ProbeGrid::near(EDGE_BLOCKS);
    let edge = edge();
    // Probe 13 is block (1, 1, 1); its centre is the only one at (24, 24, 24).
    let probe = 13;
    let centre = grid.centre(probe);
    let target = [
        centre[0].floor() as u32,
        centre[1].floor() as u32,
        centre[2].floor() as u32,
    ];
    let index = cell_index(edge, target[0], target[1], target[2]);
    let mut solid = vec![false; edge * edge * edge];
    solid[index] = true;

    let mut volume = full_bake(chunk, grid, &solid);
    assert!(!volume.is_valid(probe), "the centre starts in solid");

    let mut affected = Vec::new();
    let reach = probes_reaching(
        chunk,
        grid,
        target,
        |x, y, z| solid[cell_index(edge, x, y, z)],
        &mut affected,
    );
    assert!(
        affected.contains(&probe),
        "the edited-centre probe is selected"
    );
    assert_eq!(reach.affected, affected.len());

    solid[index] = false;
    let stats = volume.rebake(chunk, &affected, |x, y, z| solid[cell_index(edge, x, y, z)]);
    assert!(volume.is_valid(probe), "the probe becomes valid");
    assert_eq!(stats.valid, 1, "only the centre probe changed validity");
    assert_eq!(volume.sh_bytes(), full_bake(chunk, grid, &solid).sh_bytes());

    let reach = probes_reaching(
        chunk,
        grid,
        target,
        |x, y, z| solid[cell_index(edge, x, y, z)],
        &mut affected,
    );
    assert!(reach.affected >= 1);
    solid[index] = true;
    volume.rebake(chunk, &affected, |x, y, z| solid[cell_index(edge, x, y, z)]);
    assert!(
        !volume.is_valid(probe),
        "placing it back invalidates the probe"
    );
    let full = full_bake(chunk, grid, &solid);
    assert_eq!(volume.sh_bytes(), full.sh_bytes());
    assert_eq!(volume.validity_bytes(), full.validity_bytes());
}

#[test]
fn probe_update_runs_on_the_compiled_chunk_edge() {
    let grid = ProbeGrid::for_chunk(voxel_mesh::probe::ProbeLod::Near);
    let edge = grid.scene_edge_voxels();
    let chunk = ChunkPos::new(0, 0, 0);
    let block = BLOCK_DIM as u32;
    let mut solid = vec![false; edge * edge * edge];
    // Probe rays cover the upper hemisphere only, so the visible surface is a
    // roof, not a floor.
    for z in 0..edge as u32 {
        for x in 0..edge as u32 {
            for y in edge as u32 - block..edge as u32 {
                solid[cell_index(edge, x, y, z)] = true;
            }
        }
    }

    let mut volume = bake(chunk, grid, |x, y, z| solid[cell_index(edge, x, y, z)]);
    let targets = first_hit_targets(chunk, grid, &solid, edge);
    assert!(!targets.is_empty(), "the roof must be visible from a probe");
    let target = targets[0];
    let index = cell_index(edge, target[0], target[1], target[2]);
    let mut affected = Vec::new();
    let reach = probes_reaching(
        chunk,
        grid,
        target,
        |x, y, z| solid[cell_index(edge, x, y, z)],
        &mut affected,
    );
    assert!(reach.affected > 0, "the roof surface is visible");
    solid[index] = false;
    let stats = volume.rebake(chunk, &affected, |x, y, z| solid[cell_index(edge, x, y, z)]);
    assert_eq!(stats.probes, affected.len());

    let full = bake(chunk, grid, |x, y, z| solid[cell_index(edge, x, y, z)]);
    assert_eq!(volume.sh_bytes(), full.sh_bytes());
    assert_eq!(volume.validity_bytes(), full.validity_bytes());
}

#[test]
fn probe_update_is_deterministic_across_runs_threads_and_order() {
    let chunk = ChunkPos::new(2, -4, 5);
    let grid = ProbeGrid::near(EDGE_BLOCKS);
    let edge = edge();
    let mut solid = initial_fixture();
    let targets = first_hit_targets(chunk, grid, &solid, edge);
    let target = targets[0];
    let index = cell_index(edge, target[0], target[1], target[2]);

    let base = full_bake(chunk, grid, &solid);
    let mut affected = Vec::new();
    probes_reaching(
        chunk,
        grid,
        target,
        |x, y, z| solid[cell_index(edge, x, y, z)],
        &mut affected,
    );
    solid[index] = false;

    let mut forward = Probes::new(base.clone(), affected.clone(), chunk, solid.clone());
    forward.apply();
    let expected = forward.volume.sh_bytes();

    // Reversing the index order must not change a byte: probes are independent.
    let mut reversed = affected.clone();
    reversed.reverse();
    let mut backward = Probes::new(base.clone(), reversed, chunk, solid.clone());
    backward.apply();
    assert_eq!(backward.volume.sh_bytes(), expected, "index order");

    // Repeated runs and another thread must produce the same bytes.
    for run in 0..3 {
        let mut again = Probes::new(base.clone(), affected.clone(), chunk, solid.clone());
        again.apply();
        assert_eq!(again.volume.sh_bytes(), expected, "run {run}");
    }
    let worker = {
        let solid = solid.clone();
        let affected = affected.clone();
        let base = base.clone();
        std::thread::spawn(move || {
            let mut job = Probes::new(base, affected, chunk, solid);
            job.apply();
            job.volume.sh_bytes()
        })
    };
    assert_eq!(worker.join().expect("update thread panicked"), expected);
}

/// Small helper so the determinism test can move a volume across threads.
struct Probes {
    volume: ProbeVolume,
    indices: Vec<usize>,
    chunk: ChunkPos,
    solid: Vec<bool>,
}

impl Probes {
    fn new(volume: ProbeVolume, indices: Vec<usize>, chunk: ChunkPos, solid: Vec<bool>) -> Self {
        Self {
            volume,
            indices,
            chunk,
            solid,
        }
    }

    fn apply(&mut self) {
        let edge = edge();
        self.volume.rebake(self.chunk, &self.indices, |x, y, z| {
            self.solid[cell_index(edge, x, y, z)]
        });
    }
}

#[test]
fn probe_update_first_solid_matches_the_reference() {
    let chunk = ChunkPos::new(1, 2, -3);
    let grid = ProbeGrid::near(EDGE_BLOCKS);
    let solid = initial_fixture();
    let edge = edge();
    let solid_at = |x: u32, y: u32, z: u32| solid[cell_index(edge, x, y, z)];

    for probe in (0..grid.len()).step_by(4) {
        let centre = grid.centre(probe);
        let centre_cell = [
            centre[0].floor() as u32,
            centre[1].floor() as u32,
            centre[2].floor() as u32,
        ];
        if solid_at(centre_cell[0], centre_cell[1], centre_cell[2]) {
            continue;
        }
        let rotation = probe_rotation(probe_seed(chunk, grid.lod(), probe as u32));
        for ray in 0..PROBE_RAYS {
            let direction = hemisphere_ray(ray, rotation);
            assert_eq!(
                first_solid_on_ray(edge, centre, direction, solid_at),
                oracle::first_solid_on_ray(edge, centre, direction, solid_at),
                "probe {probe} ray {ray}"
            );
        }
    }
}
