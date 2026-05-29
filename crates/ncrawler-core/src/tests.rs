//! Store round-trip + layout tests.

use chrono::{TimeZone, Utc};
use ncrawler_spi::{Artifact, Asset, Item, ItemKind};

use crate::{dir_name, safe, ArtifactStore, StoreError};

fn sample() -> Artifact {
    let mut a = Artifact::new(
        "grafana",
        "abc123",
        Utc.with_ymd_and_hms(2026, 5, 29, 14, 22, 1).unwrap(),
    );
    a.items.push(Item {
        id: "panel-2".into(),
        kind: ItemKind::Panel,
        title: Some("CPU".into()),
        text: "cpu graph".into(),
        data: Some(serde_json::json!({"v": 1})),
        tags: vec!["cpu".into()],
    });
    a.assets.push(Asset {
        path: "assets/panel-2.png".into(),
        mime: "image/png".into(),
        label: "CPU".into(),
        item_id: Some("panel-2".into()),
    });
    a
}

#[test]
fn dir_name_is_the_index() {
    let ts = Utc.with_ymd_and_hms(2026, 5, 29, 14, 22, 1).unwrap();
    assert_eq!(
        dir_name(ts, "grafana", "abc123"),
        "2026-05-29T14-22-01Z__grafana__abc123"
    );
}

#[test]
fn safe_sanitises_targets() {
    assert_eq!(safe("https://x.io/d/uid?a=1"), "https---x.io-d-uid-a-1");
    assert_eq!(safe(""), "-");
}

#[test]
fn write_read_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    let a = sample();
    let adir = store.write(&a).unwrap();

    let back = store.read(&adir).unwrap();
    assert_eq!(back.source, "grafana");
    assert_eq!(back.items.len(), 1);
    assert_eq!(back.assets[0].item_id.as_deref(), Some("panel-2"));
    assert!(adir.join("assets").is_dir());
}

#[cfg(unix)]
#[test]
fn dirs_are_0700() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    let adir = store.write(&sample()).unwrap();
    let mode = std::fs::metadata(&adir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn latest_symlink_points_at_newest() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());

    let mut older = sample();
    older.fetched_at = Utc.with_ymd_and_hms(2026, 5, 29, 10, 0, 0).unwrap();
    store.write(&older).unwrap();

    let mut newer = sample();
    newer.fetched_at = Utc.with_ymd_and_hms(2026, 5, 29, 14, 0, 0).unwrap();
    let newer_dir = store.write(&newer).unwrap();

    let link = store.latest_link("grafana", "abc123");
    let resolved = std::fs::canonicalize(&link).unwrap();
    assert_eq!(resolved, std::fs::canonicalize(&newer_dir).unwrap());
    // Reading through the symlink works.
    assert_eq!(store.read(&link).unwrap().target, "abc123");
}

#[test]
fn list_filters_by_source_and_since() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());

    let mut g = sample();
    g.fetched_at = Utc::now();
    store.write(&g).unwrap();

    let mut old = sample();
    old.source = "spider".into();
    old.target = "x".into();
    old.fetched_at = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
    store.write(&old).unwrap();

    let all = store.list(None, None).unwrap();
    assert_eq!(all.len(), 2);
    // newest first
    assert_eq!(all[0].source, "grafana");

    let just_grafana = store.list(Some("grafana"), None).unwrap();
    assert_eq!(just_grafana.len(), 1);

    let recent = store
        .list(None, Some(Utc::now() - chrono::Duration::hours(1)))
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].source, "grafana");
}

#[test]
fn list_on_empty_store_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path().join("does-not-exist"));
    assert!(store.list(None, None).unwrap().is_empty());
}

#[test]
fn rejects_unknown_major_schema() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    let adir = store.write(&sample()).unwrap();

    // Bump the on-disk schema beyond what we support.
    let path = adir.join("artifact.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["schema_version"] = serde_json::json!(999);
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();

    match store.read(&adir) {
        Err(StoreError::UnsupportedSchema { found, .. }) => assert_eq!(found, 999),
        other => panic!("expected UnsupportedSchema, got {other:?}"),
    }
}
