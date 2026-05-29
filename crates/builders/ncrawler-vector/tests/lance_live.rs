//! Integration test against a real on-disk LanceDB directory.
//!
//! Gated on `RUN_LIVE_TESTS=1` (LanceDB writes real files and pulls a heavy
//! dependency tree) and on the `store-lance` feature.
#![cfg(feature = "store-lance")]

use std::time::{SystemTime, UNIX_EPOCH};

use ncrawler_vector::{LanceStore, VectorRecord, VectorStore};

fn rec(item_id: &str, seq: usize, text: &str, v: [f32; 4]) -> VectorRecord {
    VectorRecord {
        source: "grafana".into(),
        target: "abc123".into(),
        item_id: item_id.into(),
        seq,
        text: text.into(),
        vector: v.to_vec(),
    }
}

fn unique_dir() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("ncrawler-lance-{}-{}", std::process::id(), nanos))
}

#[tokio::test]
async fn lance_roundtrip_and_upsert_idempotency() {
    if std::env::var("RUN_LIVE_TESTS").is_err() {
        eprintln!("skipping: set RUN_LIVE_TESTS=1 to run the LanceDB integration test");
        return;
    }

    let dir = unique_dir();
    let path = dir.to_str().unwrap();
    let store = LanceStore::open(path, 4).await.expect("open lance store");

    // First scrape: two chunks for panel-1, one for panel-2.
    store
        .upsert(&[
            rec("panel-1", 0, "cpu high", [0.1, 0.2, 0.3, 0.4]),
            rec("panel-1", 1, "cpu still high", [0.2, 0.2, 0.2, 0.2]),
            rec("panel-2", 0, "mem ok", [0.9, 0.0, 0.0, 0.1]),
        ])
        .await
        .expect("first upsert");
    assert_eq!(store.count().await.unwrap(), 3);

    // Re-scrape identical: no duplication.
    store
        .upsert(&[
            rec("panel-1", 0, "cpu high", [0.1, 0.2, 0.3, 0.4]),
            rec("panel-1", 1, "cpu still high", [0.2, 0.2, 0.2, 0.2]),
            rec("panel-2", 0, "mem ok", [0.9, 0.0, 0.0, 0.1]),
        ])
        .await
        .expect("idempotent re-upsert");
    assert_eq!(
        store.count().await.unwrap(),
        3,
        "re-scrape must not duplicate"
    );

    // Re-scrape panel-1 with fewer chunks: stale chunk dropped, panel-2 kept.
    store
        .upsert(&[rec("panel-1", 0, "cpu merged", [0.5, 0.5, 0.5, 0.5])])
        .await
        .expect("shrinking re-upsert");
    assert_eq!(
        store.count().await.unwrap(),
        2,
        "stale chunk removed; panel-2 survives"
    );

    std::fs::remove_dir_all(&dir).ok();
}
