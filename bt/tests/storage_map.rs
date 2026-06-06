mod fixtures;

use bt::metainfo::parse;
use bt::storage::{Segment, Storage};
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../scratch/tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn multi_storage(dir: &PathBuf) -> Storage {
    let meta = parse(&fixtures::multi_file_torrent()).unwrap();
    Storage::from_metainfo(&meta, dir)
}

#[test]
fn paths_resolve_under_name() {
    let dir = scratch("paths");
    let storage = multi_storage(&dir);
    assert_eq!(storage.paths()[0], dir.join("demo-dir/a.txt"));
    assert_eq!(storage.paths()[1], dir.join("demo-dir/sub/b.txt"));
}

#[test]
fn single_file_path_is_name() {
    let dir = scratch("single-path");
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    let storage = Storage::from_metainfo(&meta, &dir);
    assert_eq!(storage.paths(), &[dir.join("demo.bin")]);
}

#[test]
fn segments_split_across_files() {
    let dir = scratch("segments");
    let storage = multi_storage(&dir);
    assert_eq!(
        storage.segments(0, 9),
        vec![
            Segment { file: 0, offset: 0, length: 5 },
            Segment { file: 1, offset: 0, length: 4 },
        ]
    );
    assert_eq!(
        storage.segments(3, 4),
        vec![
            Segment { file: 0, offset: 3, length: 2 },
            Segment { file: 1, offset: 0, length: 2 },
        ]
    );
    assert_eq!(storage.segments(5, 4), vec![Segment { file: 1, offset: 0, length: 4 }]);
}

#[test]
fn piece_size_handles_short_last_piece() {
    let dir = scratch("piece-size");
    let storage = multi_storage(&dir);
    assert_eq!(storage.piece_size(0), 9);
    assert_eq!(storage.piece_size(1), 0);
}

#[test]
fn allocate_creates_files_with_lengths() {
    let dir = scratch("allocate");
    let storage = multi_storage(&dir);
    storage.allocate().unwrap();
    assert_eq!(std::fs::metadata(&storage.paths()[0]).unwrap().len(), 5);
    assert_eq!(std::fs::metadata(&storage.paths()[1]).unwrap().len(), 4);
}

#[test]
fn write_spanning_block_lands_in_both_files() {
    let dir = scratch("write-span");
    let storage = multi_storage(&dir);
    storage.allocate().unwrap();
    storage.write_block(0, 0, b"alphabeta").unwrap();
    assert_eq!(std::fs::read(&storage.paths()[0]).unwrap(), b"alpha");
    assert_eq!(std::fs::read(&storage.paths()[1]).unwrap(), b"beta");
    assert_eq!(storage.read_piece(0).unwrap(), b"alphabeta");
}

#[test]
fn partial_writes_compose() {
    let dir = scratch("write-parts");
    let storage = multi_storage(&dir);
    storage.allocate().unwrap();
    storage.write_block(0, 0, b"alph").unwrap();
    storage.write_block(0, 4, b"abeta").unwrap();
    assert_eq!(storage.read_block(0, 0, 9).unwrap(), b"alphabeta");
}

#[test]
fn out_of_range_io_rejected() {
    let dir = scratch("oob");
    let storage = multi_storage(&dir);
    storage.allocate().unwrap();
    assert!(storage.write_block(0, 8, b"xx").is_err());
    assert!(storage.read_block(0, 0, 10).is_err());
    assert!(storage.write_block(1, 0, b"x").is_err());
}
