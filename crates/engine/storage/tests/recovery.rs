//! Corrupt region logs: recovery skips bad records, keeps later ones, and
//! counts what it lost.
use std::fs;
use std::path::PathBuf;

use voxel_storage::CHUNK_VOLUME;
use voxel_storage::ChunkPos;
use voxel_storage::dag::Svdag;
use voxel_storage::disk::RegionStore;
use voxel_storage::scan::region_path;

/// Record header size, mirrored from the on-disk format.
const HDR: usize = 16;

fn tmp(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "vstore-recovery-{name}-{}-{stamp}",
        std::process::id()
    ))
}

fn solid(material: u16) -> Svdag {
    Svdag::from_dense(&vec![material; CHUNK_VOLUME])
}

/// Declared payload length of the record starting at `at`.
fn record_len(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize
}

fn record_end(bytes: &[u8], at: usize) -> usize {
    at + HDR + record_len(bytes, at)
}

fn solid_material(store: &mut RegionStore, p: ChunkPos) -> Option<u16> {
    store.load(p).unwrap().map(|(dag, _)| dag.get(0, 0, 0))
}

#[test]
fn a_corrupt_middle_record_does_not_discard_later_records() {
    let dir = tmp("middle");
    let a = ChunkPos::new(0, 0, 0);
    let b = ChunkPos::new(1, 0, 0);
    let c = ChunkPos::new(2, 0, 0);
    {
        let mut store = RegionStore::open(&dir).unwrap();
        store.put_dag(a, &solid(1)).unwrap();
        store.put_dag(b, &solid(2)).unwrap();
        store.put_dag(c, &solid(3)).unwrap();
        store.sync().unwrap();
    }

    let path = region_path(&dir, (0, 0, 0));
    let mut bytes = fs::read(&path).unwrap();
    let b_start = record_end(&bytes, 0);
    let b_len = record_len(&bytes, b_start);
    // Flip the first payload byte of `b`, leaving its header intact.
    bytes[b_start + HDR] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();
    let total = bytes.len() as u64;

    let mut store = RegionStore::open(&dir).unwrap();
    assert_eq!(
        solid_material(&mut store, a),
        Some(1),
        "record before damage"
    );
    assert_eq!(
        store.recovered_bytes(),
        (HDR + b_len) as u64,
        "only the corrupt record's bytes are counted"
    );
    assert_eq!(
        solid_material(&mut store, b),
        None,
        "the damaged record is gone"
    );
    assert_eq!(
        solid_material(&mut store, c),
        Some(3),
        "the record after the damage survives"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        total,
        "a middle corruption does not truncate the file"
    );

    // Appending after recovery must land after the last valid record and stay
    // loadable on the next open.
    store.put_dag(a, &solid(4)).unwrap();
    store.sync().unwrap();
    drop(store);

    let mut reopened = RegionStore::open(&dir).unwrap();
    assert_eq!(solid_material(&mut reopened, a), Some(4));
    assert_eq!(solid_material(&mut reopened, c), Some(3));
    assert!(
        reopened.recovered_bytes() > 0,
        "the damaged record is still in the log and skipped again"
    );
    drop(reopened);

    fs::remove_dir_all(dir).ok();
}

#[test]
fn a_torn_tail_is_truncated_and_counted() {
    let dir = tmp("tail");
    let p = ChunkPos::new(0, 0, 0);
    {
        let mut store = RegionStore::open(&dir).unwrap();
        store.put_dag(p, &solid(1)).unwrap();
        store.sync().unwrap();
    }

    let path = region_path(&dir, (0, 0, 0));
    let clean = fs::metadata(&path).unwrap().len();
    let garbage = [0xde, 0xad, 0xbe, 0xef, 0x00];
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&garbage).unwrap();
    }

    let mut store = RegionStore::open(&dir).unwrap();
    assert_eq!(solid_material(&mut store, p), Some(1));
    assert_eq!(store.recovered_bytes(), garbage.len() as u64);
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        clean,
        "the torn tail is cut back to the last valid record"
    );
    drop(store);

    fs::remove_dir_all(dir).ok();
}

#[test]
fn a_damaged_record_is_not_resurrected_by_a_later_dag() {
    // A corrupt record followed by a valid one for the same cell: the later
    // record wins, exactly as if the corrupt one had never been written.
    let dir = tmp("same-cell");
    let p = ChunkPos::new(0, 0, 0);
    {
        let mut store = RegionStore::open(&dir).unwrap();
        store.put_dag(p, &solid(1)).unwrap();
        store.put_dag(p, &solid(2)).unwrap();
        store.sync().unwrap();
    }

    let path = region_path(&dir, (0, 0, 0));
    let mut bytes = fs::read(&path).unwrap();
    let second = record_end(&bytes, 0);
    bytes[second + HDR] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();

    let mut store = RegionStore::open(&dir).unwrap();
    assert_eq!(solid_material(&mut store, p), Some(1));
    assert!(store.recovered_bytes() > 0);
    drop(store);

    fs::remove_dir_all(dir).ok();
}

#[test]
fn a_region_with_no_records_is_not_created_by_inspection() {
    let dir = tmp("empty");
    let store = RegionStore::open(&dir).unwrap();
    assert_eq!(store.recovered_bytes(), 0);
    assert!(!region_path(&dir, (0, 0, 0)).exists());
    fs::remove_dir_all(dir).ok();
}
