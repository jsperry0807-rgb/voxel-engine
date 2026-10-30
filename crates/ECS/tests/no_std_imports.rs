//! §5 — `src/` must route collections through `crate::prelude` and avoid
//! thread/synchronization primitives. The only allowed exceptions are the
//! `std::sync::atomic` world-id counter and `std::thread::panicking()`, which
//! the specification itself requires for `Drop` guards.

use std::path::Path;

fn collect_sources(dir: &Path, out: &mut String) {
    for entry in std::fs::read_dir(dir).expect("read src directory") {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push_str(&std::fs::read_to_string(&path).expect("read source file"));
            out.push('\n');
        }
    }
}

#[test]
fn src_avoids_std_collections() {
    let mut source = String::new();
    collect_sources(Path::new("src"), &mut source);
    assert!(
        !source.contains("use std::collections"),
        "src/ must use crate::prelude collections, not std::collections"
    );
    assert!(
        !source.contains("std::collections::"),
        "src/ must not reference std::collections paths"
    );
}

#[test]
fn src_avoids_threading_and_locks() {
    let mut source = String::new();
    collect_sources(Path::new("src"), &mut source);
    for forbidden in [
        "std::thread::spawn",
        "std::thread::scope",
        "std::sync::Mutex",
        "std::sync::RwLock",
        "std::sync::Arc",
        "std::sync::mpsc",
        "std::sync::OnceLock",
    ] {
        assert!(
            !source.contains(forbidden),
            "src/ must not use `{forbidden}`"
        );
    }
}

#[test]
fn src_uses_the_documented_allowances() {
    let mut source = String::new();
    collect_sources(Path::new("src"), &mut source);
    assert!(source.contains("std::thread::panicking"));
    assert!(source.contains("std::sync::atomic"));
}
