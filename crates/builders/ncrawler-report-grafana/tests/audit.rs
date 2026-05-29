//! Fixture-driven tests for `--mode audit` (REPORT §5).
//!
//! Each check class is exercised on both its **present** and **absent**
//! paths against synthetic artifacts laid out in a temp store, driven
//! through the real `build_report` entry point (Mode::Audit) and asserted
//! against the rendered `REPORT.md`. A final test pins the offline
//! invariant: the audit source carries no `reqwest::Client`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use serde_json::{json, Value};

use ncrawler_report_grafana::{build_report, Mode, ReportOptions, REPORT_FILENAME};
use ncrawler_spi::Cancel;

struct NeverCancel;
impl Cancel for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn cancelled<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(std::future::pending())
    }
}

/// Default datasource set: a SQL datasource (referenced by the happy-path
/// fixtures) and an InfluxDB that nothing references (unused).
fn datasources() -> Value {
    json!([
        { "name": "PostgreSQL", "type": "postgres", "uid": "pg1", "isDefault": true },
        { "name": "InfluxDB", "type": "influxdb", "uid": "inf1", "isDefault": false }
    ])
}

fn sidecar(datasources: Value) -> Value {
    json!({
        "schema_version": 1,
        "host": "audit.test",
        "fetched_at": "2026-05-29T06:00:00Z",
        "search": [],
        "datasources": datasources,
        "instance": { "version": "7.5.17", "edition": "OSS", "rendererAvailable": false },
        "folders": []
    })
}

/// Build a per-dashboard artifact from a dashboard JSON object + optional
/// panel response items + folder title.
fn artifact(uid: &str, dashboard: Value, items: Value) -> Value {
    json!({
        "schema_version": 1,
        "source": "grafana",
        "target": uid,
        "fetched_at": "2026-05-29T06:00:00Z",
        "items": items,
        "assets": [],
        "meta": {
            "dashboard": {
                "meta": { "folderTitle": dashboard.get("__folder").and_then(Value::as_str).unwrap_or("") },
                "dashboard": dashboard
            },
            "annotations": []
        }
    })
}

/// Lay out `<tmp>/sidecar/instance.json` + `<tmp>/<uid>/artifact.json`,
/// run the audit, and return the rendered REPORT.md.
fn run(test: &str, sidecar_json: Value, dashboards: Vec<(&str, Value)>) -> String {
    let tmp = std::env::temp_dir().join(format!("ncrawler-audit-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let sidecar_dir = tmp.join("sidecar");
    std::fs::create_dir_all(&sidecar_dir).unwrap();
    std::fs::write(
        sidecar_dir.join("instance.json"),
        serde_json::to_vec_pretty(&sidecar_json).unwrap(),
    )
    .unwrap();

    let mut dirs: Vec<PathBuf> = Vec::new();
    for (uid, art) in &dashboards {
        let d = tmp.join(uid);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("artifact.json"),
            serde_json::to_vec_pretty(art).unwrap(),
        )
        .unwrap();
        dirs.push(d);
    }

    let opts = ReportOptions {
        mode: Mode::Audit,
        data: false,
        window: "now-6h".to_owned(),
        redact: true,
    };
    build_report(
        &sidecar_dir,
        &dirs,
        "--all",
        dashboards.len(),
        dashboards.len(),
        &opts,
        &NeverCancel,
    )
    .expect("audit build");
    std::fs::read_to_string(sidecar_dir.join(REPORT_FILENAME)).unwrap()
}

/// A clean dashboard that references the SQL datasource, sits in a folder,
/// is tagged, and whose query returned rows — the "absent" path for several
/// checks at once.
fn clean_dashboard() -> Value {
    json!({
        "uid": "clean", "title": "Clean", "tags": ["ok"], "__folder": "Sites",
        "templating": { "list": [] },
        "panels": [
            { "id": 1, "title": "P1", "type": "table", "datasource": { "type": "postgres", "uid": "pg1" },
              "targets": [ { "refId": "A", "rawSql": "SELECT 1" } ] }
        ]
    })
}

fn clean_items() -> Value {
    json!([
        { "id": "panel-1", "kind": "panel", "title": "P1", "text": "", "tags": [],
          "data": { "results": { "A": { "frames": [ { "schema": { "fields": [{"name":"n"}] }, "data": { "values": [[1, 2]] } } ] } } } }
    ])
}

#[test]
fn dead_datasource_reference() {
    let bad = json!({
        "uid": "dead", "title": "Dead DS", "tags": ["t"], "__folder": "Sites",
        "templating": { "list": [] },
        "panels": [
            { "id": 1, "title": "Ghost", "type": "graph", "datasource": { "uid": "ghost-uid" },
              "targets": [] }
        ]
    });
    let md = run(
        "dead",
        sidecar(datasources()),
        vec![
            ("clean", artifact("clean", clean_dashboard(), clean_items())),
            ("dead", artifact("dead", bad, json!([]))),
        ],
    );
    // Present: the ghost reference is flagged on the dead dashboard.
    assert!(
        md.contains("dead-datasource"),
        "missing dead-datasource:\n{md}"
    );
    assert!(md.contains("ghost-uid"));
    // Absent: the clean dashboard's valid postgres ref is not flagged.
    assert!(!md.contains("PostgreSQL` not present"));
}

#[test]
fn broken_query_error_frame() {
    let dash = json!({
        "uid": "broken", "title": "Broken", "tags": ["t"], "__folder": "Sites",
        "templating": { "list": [] },
        "panels": [
            { "id": 7, "title": "Bad SQL", "type": "table", "datasource": { "type": "postgres" },
              "targets": [ { "refId": "A", "rawSql": "SELECT * FROM metric_table" } ] }
        ]
    });
    let items = json!([
        { "id": "panel-7", "kind": "panel", "title": "Bad SQL", "text": "", "tags": [],
          "data": { "results": { "A": { "error": "relation \"metric_table\" does not exist", "status": 500 } } } }
    ]);
    let md = run(
        "broken",
        sidecar(datasources()),
        vec![
            ("broken", artifact("broken", dash, items)),
            ("clean", artifact("clean", clean_dashboard(), clean_items())),
        ],
    );
    assert!(md.contains("broken-query"), "missing broken-query:\n{md}");
    assert!(md.contains("metric_table"));
    // Absent: clean dashboard produced no broken-query row for its panel.
    assert_eq!(md.matches("broken-query").count(), 1);
}

#[test]
fn empty_panel_zero_rows() {
    let dash = json!({
        "uid": "empty", "title": "Empty", "tags": ["t"], "__folder": "Sites",
        "templating": { "list": [] },
        "panels": [
            { "id": 2, "title": "Nothing", "type": "table", "datasource": { "type": "postgres" },
              "targets": [ { "refId": "A", "rawSql": "SELECT 1 WHERE false" } ] }
        ]
    });
    let items = json!([
        { "id": "panel-2", "kind": "panel", "title": "Nothing", "text": "", "tags": [],
          "data": { "results": { "A": { "frames": [ { "schema": { "fields": [{"name":"n"}] }, "data": { "values": [[]] } } ] } } } }
    ]);
    let md = run(
        "empty",
        sidecar(datasources()),
        vec![
            ("empty", artifact("empty", dash, items)),
            ("clean", artifact("clean", clean_dashboard(), clean_items())),
        ],
    );
    assert!(md.contains("empty-panel"), "missing empty-panel:\n{md}");
    assert!(md.contains("0 rows"));
    // Absent: clean dashboard returned rows, so exactly one empty-panel.
    assert_eq!(md.matches("empty-panel").count(), 1);
}

#[test]
fn duplicate_dashboards_match_on_fingerprint_not_title() {
    // Two IDENTICAL-CONTENT dashboards with DIFFERENT titles → duplicates.
    let body = |uid: &str, title: &str| {
        json!({
            "uid": uid, "title": title, "tags": ["t"], "__folder": "Sites",
            "templating": { "list": [ { "name": "v", "type": "query" } ] },
            "panels": [
                { "id": 1, "title": "Same", "type": "graph", "datasource": { "type": "postgres" },
                  "targets": [ { "refId": "A", "rawSql": "SELECT kwh FROM meters" } ] }
            ]
        })
    };
    // Two SAME-TITLE dashboards with DIFFERENT content → NOT duplicates.
    let same_title = |uid: &str, sql: &str| {
        json!({
            "uid": uid, "title": "Twin Title", "tags": ["t"], "__folder": "Sites",
            "templating": { "list": [] },
            "panels": [
                { "id": 1, "title": "X", "type": "graph", "datasource": { "type": "postgres" },
                  "targets": [ { "refId": "A", "rawSql": sql } ] }
            ]
        })
    };
    let md = run(
        "dups",
        sidecar(datasources()),
        vec![
            ("dupA", artifact("dupA", body("dupA", "Alpha"), json!([]))),
            ("dupB", artifact("dupB", body("dupB", "Bravo"), json!([]))),
            (
                "twin1",
                artifact("twin1", same_title("twin1", "SELECT a"), json!([])),
            ),
            (
                "twin2",
                artifact("twin2", same_title("twin2", "SELECT b"), json!([])),
            ),
        ],
    );
    // Present: dupA & dupB share a fingerprint despite different titles.
    assert!(
        md.contains("duplicate-dashboard"),
        "missing duplicate:\n{md}"
    );
    assert!(md.contains("duplicate of `dupA`"));
    // Absent: identical TITLES (twin1/twin2) with different SQL are NOT
    // flagged — fingerprint-only matching, never title similarity.
    assert!(
        !md.contains("twin1"),
        "twin dashboards must not be flagged:\n{md}"
    );
    assert!(!md.contains("twin2"));
}

#[test]
fn orphan_no_folder_untagged() {
    let orphan = json!({
        "uid": "orphan", "title": "Orphan", "tags": [],
        "templating": { "list": [] },
        "panels": [ { "id": 1, "title": "P", "type": "graph", "datasource": { "type": "postgres" }, "targets": [] } ]
    });
    let md = run(
        "orphan",
        sidecar(datasources()),
        vec![
            ("orphan", artifact("orphan", orphan, json!([]))),
            ("clean", artifact("clean", clean_dashboard(), clean_items())),
        ],
    );
    assert!(md.contains("orphan"), "missing orphan:\n{md}");
    assert!(md.contains("in no folder and untagged"));
    // Absent: the clean dashboard has a folder + tag, so only one orphan row.
    assert_eq!(md.matches("| orphan |").count(), 1);
}

#[test]
fn unused_datasource() {
    let md = run(
        "unused",
        sidecar(datasources()),
        vec![("clean", artifact("clean", clean_dashboard(), clean_items()))],
    );
    // Present: InfluxDB is configured but referenced by no panel.
    assert!(md.contains("unused-datasource"), "missing unused:\n{md}");
    assert!(md.contains("InfluxDB"));
    // Absent: PostgreSQL is referenced by the clean panel, so not unused.
    assert!(!md.contains("`PostgreSQL` (postgres) is configured but referenced by no"));
}

#[test]
fn blank_and_constant_variables() {
    let dash = json!({
        "uid": "vars", "title": "Vars", "tags": ["t"], "__folder": "Sites",
        "templating": { "list": [
            { "name": "empty_const", "type": "constant", "current": { "value": "" }, "query": "" },
            { "name": "hardcoded", "type": "constant", "current": { "value": "WH-A" }, "query": "WH-A" },
            { "name": "normal", "type": "query" }
        ] },
        "panels": [ { "id": 1, "title": "P", "type": "graph", "datasource": { "type": "postgres" }, "targets": [] } ]
    });
    let md = run(
        "vars",
        sidecar(datasources()),
        vec![("vars", artifact("vars", dash, json!([])))],
    );
    // Present: empty constant → blank-variable; hardcoded constant →
    // constant-variable.
    assert!(
        md.contains("blank-variable"),
        "missing blank-variable:\n{md}"
    );
    assert!(md.contains("empty_const"));
    assert!(md.contains("constant-variable"));
    assert!(md.contains("hardcoded"));
    // Absent: the plain query variable produces neither finding.
    assert!(!md.contains("`normal`"));
}

#[test]
fn findings_grouped_by_severity_deterministic() {
    // A dashboard that triggers error + warn + info simultaneously.
    let dash = json!({
        "uid": "mix", "title": "Mixed", "tags": [],
        "templating": { "list": [ { "name": "c", "type": "constant", "current": { "value": "" }, "query": "" } ] },
        "panels": [
            { "id": 1, "title": "Ghost", "type": "graph", "datasource": { "uid": "ghost" }, "targets": [] }
        ]
    });
    let md = run(
        "mix",
        sidecar(datasources()),
        vec![("mix", artifact("mix", dash, json!([])))],
    );
    let err = md.find("### error").expect("error group");
    let warn = md.find("### warn").expect("warn group");
    let info = md.find("### info").expect("info group");
    assert!(
        err < warn && warn < info,
        "severity groups out of order:\n{md}"
    );

    // Determinism: a second identical run yields byte-identical output.
    let md2 = run(
        "mix",
        sidecar(datasources()),
        vec![(
            "mix",
            artifact(
                "mix",
                json!({
                    "uid": "mix", "title": "Mixed", "tags": [],
                    "templating": { "list": [ { "name": "c", "type": "constant", "current": { "value": "" }, "query": "" } ] },
                    "panels": [ { "id": 1, "title": "Ghost", "type": "graph", "datasource": { "uid": "ghost" }, "targets": [] } ]
                }),
                json!([]),
            ),
        )],
    );
    assert_eq!(md, md2);
}

/// The audit path is offline (REPORT §5): it must NOT construct a
/// `reqwest::Client`. The audit source carries no reqwest reference at all.
#[test]
fn audit_path_uses_no_reqwest_client() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/audit.rs");
    let text = std::fs::read_to_string(&src).unwrap();
    // Ignore comment/doc lines (which legitimately discuss the invariant);
    // assert no executable line references reqwest at all.
    let code_has_reqwest = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .any(|l| l.contains("reqwest"));
    assert!(
        !code_has_reqwest,
        "audit.rs must not reference reqwest (offline audit per REPORT §5)"
    );
    // And the crate does not depend on reqwest at all.
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    assert!(
        !manifest.contains("reqwest"),
        "ncrawler-report-grafana must not depend on reqwest"
    );
}
