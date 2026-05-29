//! Golden-file + behaviour tests for the deterministic Markdown builder.

use std::future::Future;
use std::pin::Pin;

use ncrawler_report_md::{render, MarkdownBuilder, REPORT_FILENAME};
use ncrawler_spi::{Artifact, Asset, BuildCtx, Builder, Cancel, Item, ItemKind};

/// A `Cancel` that never cancels, for driving the async `build`.
struct NeverCancel;
impl Cancel for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn cancelled<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(std::future::pending())
    }
}

fn load_fixture() -> Artifact {
    let raw = include_str!("fixtures/grafana-multi-panel.artifact.json");
    serde_json::from_str(raw).expect("fixture artifact parses")
}

#[test]
fn golden_multi_panel_grafana() {
    let artifact = load_fixture();
    let expected = include_str!("fixtures/grafana-multi-panel.report.md");
    assert_eq!(render(&artifact), expected);
}

/// Two assets with the SAME label must still attach to the correct item
/// via `item_id` — proving label-matching is dead.
#[test]
fn assets_match_by_item_id_not_label() {
    let item = |id: &str| Item {
        id: id.to_owned(),
        kind: ItemKind::Panel,
        title: Some(id.to_owned()),
        text: id.to_owned(),
        data: None,
        tags: vec![],
    };
    let asset = |item_id: &str, path: &str| Asset {
        path: path.into(),
        mime: "image/png".to_owned(),
        // Identical label on purpose.
        label: "screenshot".to_owned(),
        item_id: Some(item_id.to_owned()),
    };
    let mut artifact = Artifact::new("grafana", "abc", "2026-05-29T00:00:00Z".parse().unwrap());
    artifact.items = vec![item("panel-1"), item("panel-2")];
    artifact.assets = vec![
        asset("panel-2", "assets/p2.png"),
        asset("panel-1", "assets/p1.png"),
    ];

    let md = render(&artifact);
    // Each panel embeds ITS asset, regardless of shared label or asset
    // ordering. If matching were by label, both would land on one item.
    let p1_section = md.split("## panel-1").nth(1).unwrap();
    let p1_body = p1_section.split("## panel-2").next().unwrap();
    assert!(p1_body.contains("assets/p1.png"), "panel-1 -> p1.png");
    assert!(
        !p1_body.contains("assets/p2.png"),
        "panel-1 must not get p2"
    );

    let p2_body = md.split("## panel-2").nth(1).unwrap();
    assert!(p2_body.contains("assets/p2.png"), "panel-2 -> p2.png");
}

#[tokio::test]
async fn build_writes_report_file() {
    let artifact = load_fixture();
    let dir = std::env::temp_dir().join(format!("ncrawler-md-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = BuildCtx {
        artifact_dir: dir.clone(),
        options: serde_json::Value::Null,
    };
    let out = MarkdownBuilder::new()
        .build(&artifact, &ctx, &NeverCancel)
        .await
        .expect("build succeeds");
    assert_eq!(out.files, vec![std::path::PathBuf::from(REPORT_FILENAME)]);
    let written = std::fs::read_to_string(dir.join(REPORT_FILENAME)).unwrap();
    assert_eq!(written, render(&artifact));
    std::fs::remove_dir_all(&dir).ok();
}

/// An artifact whose `data` carries a secret-ish literal (a long hex
/// token). The builder must mask it by default and emit it verbatim
/// under `--no-redact`.
fn secret_artifact() -> Artifact {
    let mut artifact = Artifact::new("grafana", "abc", "2026-05-29T00:00:00Z".parse().unwrap());
    artifact.items = vec![Item {
        id: "panel-1".to_owned(),
        kind: ItemKind::Panel,
        title: Some("Auth".to_owned()),
        text: "Bearer abcdef0123456789abcdef0123".to_owned(),
        data: Some(serde_json::json!({
            "rawSql": "SELECT * FROM t WHERE token = 'deadbeefdeadbeefdeadbeefdeadbeef'"
        })),
        tags: vec![],
    }];
    artifact
}

async fn build_to_string(artifact: &Artifact, options: serde_json::Value) -> String {
    let dir = std::env::temp_dir().join(format!(
        "ncrawler-md-redact-{}-{:p}",
        std::process::id(),
        artifact
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = BuildCtx {
        artifact_dir: dir.clone(),
        options,
    };
    MarkdownBuilder::new()
        .build(artifact, &ctx, &NeverCancel)
        .await
        .expect("build succeeds");
    let out = std::fs::read_to_string(dir.join(REPORT_FILENAME)).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    out
}

#[tokio::test]
async fn build_redacts_by_default() {
    let artifact = secret_artifact();
    // Default (options Null) and explicit `redact: true` both mask.
    for options in [
        serde_json::Value::Null,
        serde_json::json!({ "redact": true }),
    ] {
        let md = build_to_string(&artifact, options).await;
        assert!(md.contains("***REDACTED***"), "secret should be masked");
        assert!(
            !md.contains("deadbeefdeadbeefdeadbeefdeadbeef"),
            "raw hex token must not survive"
        );
        assert!(
            !md.contains("abcdef0123456789abcdef0123"),
            "raw bearer token must not survive"
        );
    }
}

#[tokio::test]
async fn no_redact_is_a_true_bypass() {
    let artifact = secret_artifact();
    let md = build_to_string(&artifact, serde_json::json!({ "redact": false })).await;
    // True bypass: byte-identical to the raw render, secrets intact, no mask.
    assert_eq!(md, render(&artifact));
    assert!(md.contains("deadbeefdeadbeefdeadbeefdeadbeef"));
    assert!(!md.contains("***REDACTED***"));
}
