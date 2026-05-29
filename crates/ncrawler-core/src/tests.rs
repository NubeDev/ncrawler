//! Store round-trip + layout tests.

use chrono::{DateTime, TimeZone, Utc};
use ncrawler_spi::{Artifact, Asset, Item, ItemKind};
use serde_json::json;

use crate::{
    dir_name, safe, ArtifactStore, FactsOrigin, InstanceSidecar, StoreError,
    INSTANCE_SCHEMA_VERSION,
};

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

// ---------------------------------------------------------------------
// Instance sidecar (REPORT §6a) — write, read, fallback, mixed store.
// ---------------------------------------------------------------------

const TS: fn() -> DateTime<Utc> = || Utc.with_ymd_and_hms(2026, 5, 29, 14, 22, 1).unwrap();

/// A representative sidecar with all four payloads populated.
fn sample_sidecar(host: &str, at: DateTime<Utc>) -> InstanceSidecar {
    let mut s = InstanceSidecar::new(host, at);
    s.search = json!([
        { "uid": "vijaYkWvz", "title": "Site Summary", "folderTitle": "Sites", "tags": ["site"] },
        { "uid": "Iw0GqiJSk", "title": "Navigation", "folderTitle": null, "tags": [] }
    ]);
    s.datasources = json!([
        { "id": 1, "uid": "pg", "name": "PostgreSQL", "type": "postgres", "isDefault": true },
        { "id": 2, "uid": "rbx", "name": "Rubix OS", "type": "grafana-rubix-os-data-source", "isDefault": false }
    ]);
    s.instance =
        json!({ "version": "7.5.17", "edition": "Open Source", "rendererAvailable": false });
    s.folders = json!([{ "uid": "f1", "title": "Sites" }]);
    s
}

/// A legacy per-dashboard artifact that still embeds `meta.search`
/// (pre-sidecar shape) — the migration-fallback source.
fn legacy_dash(uid: &str) -> Artifact {
    let mut a = Artifact::new("grafana", uid, TS());
    a.meta = json!({
        "dashboard": { "dashboard": { "uid": uid } },
        "search": [{ "uid": uid, "title": "Legacy Dash" }],
        "annotations": []
    });
    a
}

#[test]
fn instance_sidecar_write_read_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    let side = sample_sidecar("rd-esr.nube-iiot.com", TS());

    let sdir = store.write_instance("grafana", &side).unwrap();
    // On-disk layout: <root>/grafana/_instance/<host>/<ts>__instance/instance.json
    assert!(sdir.ends_with("2026-05-29T14-22-01Z__instance"));
    assert!(sdir.join("instance.json").is_file());
    assert!(sdir
        .to_string_lossy()
        .contains("grafana/_instance/rd-esr.nube-iiot.com/"));

    let back = store
        .read_instance_sidecar("grafana", "rd-esr.nube-iiot.com")
        .unwrap()
        .expect("sidecar present");
    assert_eq!(back.schema_version, INSTANCE_SCHEMA_VERSION);
    assert_eq!(back.host, "rd-esr.nube-iiot.com");
    assert_eq!(back.search[0]["uid"], "vijaYkWvz");
    assert_eq!(back.instance["version"], "7.5.17");
}

#[cfg(unix)]
#[test]
fn instance_sidecar_dir_is_0700() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    let sdir = store
        .write_instance("grafana", &sample_sidecar("h", TS()))
        .unwrap();
    let mode = std::fs::metadata(&sdir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn instance_latest_symlink_points_at_newest() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());

    let older = sample_sidecar("h", Utc.with_ymd_and_hms(2026, 5, 29, 10, 0, 0).unwrap());
    store.write_instance("grafana", &older).unwrap();
    let newer = sample_sidecar("h", Utc.with_ymd_and_hms(2026, 5, 29, 14, 0, 0).unwrap());
    let newer_dir = store.write_instance("grafana", &newer).unwrap();

    let link = store.instance_latest_link("grafana", "h");
    assert_eq!(
        std::fs::canonicalize(&link).unwrap(),
        std::fs::canonicalize(&newer_dir).unwrap()
    );
}

#[test]
fn read_instance_sidecar_absent_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    assert!(store
        .read_instance_sidecar("grafana", "nope")
        .unwrap()
        .is_none());
}

#[test]
fn rejects_unknown_instance_schema() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    let sdir = store
        .write_instance("grafana", &sample_sidecar("h", TS()))
        .unwrap();

    let path = sdir.join("instance.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["schema_version"] = json!(999);
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();

    match store.read_instance_sidecar("grafana", "h") {
        Err(StoreError::UnsupportedInstanceSchema { found, .. }) => assert_eq!(found, 999),
        other => panic!("expected UnsupportedInstanceSchema, got {other:?}"),
    }
}

#[test]
fn instance_facts_prefers_sidecar_over_legacy_meta() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    // Both a sidecar AND a legacy per-dashboard artifact exist for uid.
    store
        .write_instance("grafana", &sample_sidecar("h", TS()))
        .unwrap();
    store.write(&legacy_dash("vijaYkWvz")).unwrap();

    let facts = store
        .read_instance_facts("grafana", "h", "vijaYkWvz")
        .unwrap();
    assert!(matches!(facts.origin, FactsOrigin::Sidecar(_)));
    // The sidecar's inventory wins, not the legacy artifact's.
    assert_eq!(facts.search[0]["title"], "Site Summary");
    assert_eq!(facts.datasources[0]["type"], "postgres");
}

#[test]
fn instance_facts_falls_back_to_legacy_meta() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    // No sidecar — only a legacy artifact carrying meta.search.
    store.write(&legacy_dash("oldUid")).unwrap();

    let facts = store.read_instance_facts("grafana", "h", "oldUid").unwrap();
    match &facts.origin {
        FactsOrigin::LegacyMeta(p) => assert!(p.ends_with("artifact.json")),
        other => panic!("expected LegacyMeta, got {other:?}"),
    }
    assert_eq!(facts.search[0]["uid"], "oldUid");
    // Legacy meta never carried these — they resolve to null.
    assert!(facts.datasources.is_null());
    assert!(facts.instance.is_null());
    assert!(facts.folders.is_null());
}

#[test]
fn instance_facts_unavailable_when_neither_present() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    match store.read_instance_facts("grafana", "h", "ghost") {
        Err(StoreError::InstanceFactsUnavailable { host, uid }) => {
            assert_eq!(host, "h");
            assert_eq!(uid, "ghost");
        }
        other => panic!("expected InstanceFactsUnavailable, got {other:?}"),
    }
}

#[test]
fn mixed_store_new_sidecar_and_legacy_coexist() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());

    // host "new" was re-scraped → has a sidecar.
    store
        .write_instance("grafana", &sample_sidecar("new", TS()))
        .unwrap();
    // host "old" predates the sidecar → only a legacy artifact on disk.
    store.write(&legacy_dash("legacyUid")).unwrap();

    // New host resolves from the sidecar.
    let new_facts = store
        .read_instance_facts("grafana", "new", "anyUid")
        .unwrap();
    assert!(matches!(new_facts.origin, FactsOrigin::Sidecar(_)));
    assert_eq!(new_facts.search.as_array().unwrap().len(), 2);

    // Old host has no sidecar; it falls back to the legacy artifact.
    let old_facts = store
        .read_instance_facts("grafana", "old", "legacyUid")
        .unwrap();
    assert!(matches!(old_facts.origin, FactsOrigin::LegacyMeta(_)));
    assert_eq!(old_facts.search[0]["uid"], "legacyUid");
}

// ---------------------------------------------------------------------
// Golden: the sidecar JSON serialization is stable run-to-run.
// ---------------------------------------------------------------------

#[test]
fn sidecar_json_shape_is_stable() {
    let side = sample_sidecar("rd-esr.nube-iiot.com", TS());
    let rendered = serde_json::to_string_pretty(&side).unwrap();

    // Determinism: serializing the same sidecar twice is byte-identical.
    assert_eq!(rendered, serde_json::to_string_pretty(&side).unwrap());

    let golden_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/instance.golden.json"
    );
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(std::path::Path::new(golden_path).parent().unwrap()).unwrap();
        std::fs::write(golden_path, format!("{rendered}\n")).unwrap();
    }
    let golden = std::fs::read_to_string(golden_path)
        .expect("golden file present (run with UPDATE_GOLDEN=1 to create)");
    assert_eq!(
        rendered.trim_end(),
        golden.trim_end(),
        "sidecar JSON shape drifted from golden; re-run with UPDATE_GOLDEN=1 if intended"
    );
}
