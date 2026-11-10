use tupi_domain::index::{Index, IndexFile, IndexSymbol};
use tupi_store::Store;

fn sample_index() -> Index {
    Index {
        version: 1,
        scanned_at_unix: 0,
        files: vec![IndexFile {
            path: "src/lib.rs".into(),
            hash: "abc".into(),
            language: "rust".into(),
            symbols: vec![IndexSymbol {
                name: "alpha".into(),
                qualified: "alpha".into(),
                signature: "pub fn alpha()".into(),
                start_line: 1,
                end_line: 1,
            }],
        }],
    }
}

#[test]
fn index_round_trips_per_repo() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("tupi.redb")).unwrap();
    let index = sample_index();

    store.save_index("repo-a", &index).unwrap();

    assert_eq!(store.load_index("repo-a").unwrap(), Some(index));
    assert_eq!(store.load_index("repo-b").unwrap(), None);
}
