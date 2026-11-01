//! M2.5-07: river extraction.
//!
//! The hand-checked plane fixture pins the segment structure exactly; the
//! random fields then check the invariants the design promises: every
//! non-confluence segment reaches sea or a lake, no segment climbs, widths stay
//! in 10–200 m, and confluence links terminate.

use voxel_worldgen::hydro::{FlowDir, Hydrology};
use voxel_worldgen::rivers::{MAX_WIDTH, MIN_WIDTH, Mouth, RiverConfig, RiverNetwork, is_delta};

fn grid(rows: &[&[u16]]) -> (Vec<u16>, usize, usize) {
    let height = rows.len();
    let width = rows[0].len();
    let mut heights = Vec::with_capacity(width * height);
    for row in rows {
        assert_eq!(row.len(), width);
        heights.extend_from_slice(row);
    }
    (heights, width, height)
}

/// A single interior row sloping west: `h = 10x` inside, `100 + 10x` on the
/// frame. One channel, hand-checkable accumulation.
fn plane() -> Hydrology {
    let interior = [0u16, 10, 20, 30, 40, 50];
    let frame = [100u16, 110, 120, 130, 140, 150];
    let heights = [frame.as_slice(), interior.as_slice(), frame.as_slice()].concat();
    Hydrology::compute(heights, 6, 3, 0)
}

/// Assert the structural promises for every segment in a network.
fn assert_network_invariants(hydro: &Hydrology, network: &RiverNetwork) {
    let width = hydro.width();
    let height = hydro.height();
    for river in network.rivers() {
        assert!(!river.points.is_empty(), "empty segment");
        assert!(river.order >= 1, "order 0");
        for pair in river.points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(
                (a.x - b.x).abs() <= 1 && (a.z - b.z).abs() <= 1 && (a.x, a.z) != (b.x, b.z),
                "points are not 8-connected"
            );
            assert!(
                b.accumulation > a.accumulation,
                "accumulation does not grow downstream"
            );
            let first = a.z as usize * width + a.x as usize;
            let second = b.z as usize * width + b.x as usize;
            assert!(
                hydro.filled()[second] <= hydro.filled()[first],
                "river climbs from {} to {}",
                hydro.filled()[first],
                hydro.filled()[second]
            );
        }
        for point in &river.points {
            assert!(point.x >= 0 && point.z >= 0);
            assert!((point.x as usize) < width && (point.z as usize) < height);
            assert!(
                (MIN_WIDTH..=MAX_WIDTH).contains(&point.width),
                "width {} out of range",
                point.width
            );
            assert!(point.depth > 0);
            let index = point.z as usize * width + point.x as usize;
            assert_eq!(
                hydro.accumulation()[index],
                point.accumulation,
                "accumulation does not match the hydrology"
            );
        }

        match river.mouth {
            Mouth::Sea | Mouth::Lake | Mouth::MapEdge => {}
            Mouth::Confluence => {
                let junction = river.end().expect("confluence end");
                let downstream: Vec<_> = network
                    .rivers()
                    .iter()
                    .filter(|other| other.start() == Some(junction))
                    .collect();
                assert!(
                    !downstream.is_empty(),
                    "confluence {junction:?} has no downstream segment"
                );
                for next in downstream {
                    assert!(
                        next.order >= river.order,
                        "order falls through a confluence"
                    );
                }
            }
        }
    }
}

/// Follow confluence links from every segment and require a terminal mouth.
fn assert_all_rivers_terminate(network: &RiverNetwork) {
    let limit = network.len() + 1;
    for river in network.rivers() {
        let mut current = river;
        let mut steps = 0;
        loop {
            match current.mouth {
                Mouth::Sea | Mouth::Lake | Mouth::MapEdge => break,
                Mouth::Confluence => {
                    let junction = current.end().expect("confluence end");
                    let next = network
                        .rivers()
                        .iter()
                        .find(|other| other.start() == Some(junction))
                        .unwrap_or_else(|| panic!("no downstream segment at {junction:?}"));
                    current = next;
                }
            }
            steps += 1;
            assert!(steps <= limit, "confluence links form a cycle");
        }
    }
}

#[test]
fn rivers_plane_is_hand_checked() {
    let hydro = plane();
    let network = RiverNetwork::extract_with(
        &hydro,
        RiverConfig {
            threshold: 2,
            ..RiverConfig::default()
        },
    );

    // Rows of height x drain west; cells with accumulation >= 2 are river.
    assert_eq!(network.len(), 1, "one river on the interior row");
    for river in network.rivers() {
        assert_eq!(river.points.len(), 4, "x = 3, 2, 1, 0");
        assert_eq!(river.order, 1);
        assert_eq!(river.mouth, Mouth::Sea);
        assert!(!river.delta);
        assert_eq!(river.length_m(), 3 * 128);
        let accumulations: Vec<u32> = river.points.iter().map(|p| p.accumulation).collect();
        assert_eq!(accumulations, vec![2, 3, 4, 5]);
        for point in &river.points {
            assert_eq!(point.width, MIN_WIDTH, "small channels stay at the minimum");
        }
    }
    assert_eq!(network.max_order(), 1);
    assert_eq!(network.delta_count(), 0);
    assert_network_invariants(&hydro, &network);
    assert_all_rivers_terminate(&network);
}

#[test]
fn rivers_lake_mouth_stops_at_deep_water() {
    // A west-sloping channel with one deep pit: the river ends at the lake and
    // a new segment leaves the spill.
    // Sea 110; the channel runs 100, 104, 108 … so the west border drains.
    // A pit at x = 10 (115, above sea and below both neighbours) fills to the
    // spill at 136: a 21-unit (5.25 m) lake that ends the upstream river.
    let width = 40usize;
    let mut heights = Vec::with_capacity(width * 3);
    for z in 0..3usize {
        for x in 0..width {
            let height = if z == 1 {
                100 + (x as u16) * 4
            } else {
                900 + (x as u16) * 4
            };
            heights.push(if x == 10 && z == 1 { 115 } else { height });
        }
    }
    let hydro = Hydrology::compute(heights, width, 3, 110);
    let network = RiverNetwork::extract_with(
        &hydro,
        RiverConfig {
            threshold: 2,
            ..RiverConfig::default()
        },
    );

    assert_network_invariants(&hydro, &network);
    assert_all_rivers_terminate(&network);
    assert!(
        network
            .rivers()
            .iter()
            .any(|river| river.mouth == Mouth::Lake),
        "no river ends at the lake"
    );
    assert!(
        network
            .rivers()
            .iter()
            .any(|river| river.mouth == Mouth::Sea),
        "no river reaches the sea"
    );
}

#[test]
fn rivers_are_stable_between_runs() {
    let hydro = random_hydro(64, 48, 11, 2);
    let first = RiverNetwork::extract_with(
        &hydro,
        RiverConfig {
            threshold: 8,
            ..RiverConfig::default()
        },
    );
    let second = RiverNetwork::extract_with(
        &hydro,
        RiverConfig {
            threshold: 8,
            ..RiverConfig::default()
        },
    );
    assert_eq!(first.rivers(), second.rivers());
    assert_eq!(first.content_hash(), second.content_hash());
}

#[test]
fn rivers_network_invariants_hold_on_random_fields() {
    let mut with_rivers = 0;
    for seed in 0..6u64 {
        let hydro = random_hydro(72, 56, seed, 2);
        let network = RiverNetwork::extract_with(
            &hydro,
            RiverConfig {
                threshold: 6,
                ..RiverConfig::default()
            },
        );
        assert_network_invariants(&hydro, &network);
        assert_all_rivers_terminate(&network);
        if network.total_length_m() > 0 {
            with_rivers += 1;
        }
    }
    assert!(
        with_rivers >= 4,
        "only {with_rivers} of 6 fields had rivers"
    );
}

#[test]
fn rivers_widths_follow_accumulation() {
    let hydro = random_hydro(96, 72, 21, 3);
    let network = RiverNetwork::extract_with(
        &hydro,
        RiverConfig {
            threshold: 4,
            ..RiverConfig::default()
        },
    );
    for river in network.rivers() {
        for pair in river.points.windows(2) {
            assert!(pair[1].accumulation > pair[0].accumulation);
            assert!(
                pair[1].width >= pair[0].width,
                "width falls as accumulation grows"
            );
        }
    }
    if let Some((low, high)) = network.width_range() {
        assert!(low >= MIN_WIDTH && high <= MAX_WIDTH);
    }
}

#[test]
fn rivers_delta_rule_is_pinned() {
    assert!(is_delta(2, 0));
    assert!(is_delta(3, 2));
    assert!(!is_delta(1, 0), "a headwater stream cannot build a delta");
    assert!(!is_delta(4, 3), "a steep mouth is not a delta");
    assert!(!is_delta(0, 1));
}

#[test]
fn rivers_delta_flags_match_the_gradient_rule() {
    // Whatever the field produces, a delta flag must be justified by the same
    // rule the extractor uses.
    let hydro = random_hydro(96, 64, 5, 2);
    let network = RiverNetwork::extract_with(
        &hydro,
        RiverConfig {
            threshold: 4,
            ..RiverConfig::default()
        },
    );
    let width = hydro.width();
    for river in network.rivers() {
        if !river.delta {
            continue;
        }
        assert_eq!(river.mouth, Mouth::Sea);
        let sample = river
            .points
            .len()
            .min(voxel_worldgen::rivers::DELTA_SAMPLE_CELLS);
        assert!(sample >= 2);
        let first = &river.points[river.points.len() - sample];
        let last = &river.points[river.points.len() - 1];
        let first_index = first.z as usize * width + first.x as usize;
        let last_index = last.z as usize * width + last.x as usize;
        let drop_units =
            (hydro.filled()[first_index] as i64 - hydro.filled()[last_index] as i64).max(0);
        let drop_mm = drop_units * 250;
        let run_m = (sample as i64 - 1) * 128;
        let gradient = drop_mm / run_m;
        assert!(
            is_delta(river.order, gradient),
            "delta flag without a flat, confluent mouth (order {}, gradient {})",
            river.order,
            gradient
        );
    }
}

#[test]
fn rivers_confluence_segments_share_their_junction() {
    // On fields with confluences, the junction cell ends the upstream segment
    // and starts the downstream one.
    let mut checked = 0;
    for seed in 0..6u64 {
        let hydro = random_hydro(72, 56, seed, 2);
        let network = RiverNetwork::extract_with(
            &hydro,
            RiverConfig {
                threshold: 6,
                ..RiverConfig::default()
            },
        );
        for river in network.rivers() {
            if river.mouth != Mouth::Confluence {
                continue;
            }
            checked += 1;
            let junction = river.end().expect("confluence end");
            assert!(
                network
                    .rivers()
                    .iter()
                    .any(|other| other.start() == Some(junction)),
                "confluence junction is not a downstream start"
            );
            // The junction point is shared, not duplicated with different data.
            let downstream = network
                .rivers()
                .iter()
                .find(|other| other.start() == Some(junction))
                .expect("downstream");
            assert_eq!(river.points.last(), downstream.points.first());
        }
    }
    assert!(checked > 0, "no confluences in the sampled fields");
}

#[test]
fn rivers_empty_when_nothing_drains() {
    // A flat plateau: every cell is at or below sea level, so all cells are
    // outlets and no river forms.
    let (heights, width, height) = grid(&[&[0, 0, 0], &[0, 0, 0], &[0, 0, 0]]);
    let hydro = Hydrology::compute(heights, width, height, 0);
    let network = RiverNetwork::extract(&hydro);
    assert!(network.is_empty());
    assert_eq!(network.max_order(), 0);
    assert_eq!(network.width_range(), None);
}

#[test]
fn rivers_single_channel_flow_direction_is_checked() {
    // Every point's flow must be one of the eight compass directions (never
    // an outlet) except the last point of a terminal segment.
    let hydro = plane();
    let network = RiverNetwork::extract_with(
        &hydro,
        RiverConfig {
            threshold: 2,
            ..RiverConfig::default()
        },
    );
    let width = hydro.width();
    for river in network.rivers() {
        for point in &river.points[..river.points.len() - 1] {
            let index = point.z as usize * width + point.x as usize;
            assert!(!matches!(hydro.flow()[index], FlowDir::Outlet));
        }
        let last = river.points.last().expect("non-empty");
        let index = last.z as usize * width + last.x as usize;
        if river.mouth == Mouth::Sea {
            assert_eq!(hydro.flow()[index], FlowDir::Outlet);
        }
    }
}

/// Deterministic pseudo-random heightfield.
fn random_hydro(width: usize, height: usize, seed: u64, sea_level: u16) -> Hydrology {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut heights = Vec::with_capacity(width * height);
    for _ in 0..width * height {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        heights.push(((state >> 33) % 40) as u16);
    }
    Hydrology::compute(heights, width, height, sea_level)
}
