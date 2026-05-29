//! Golden-file tests for the deterministic Grafana report builder.
//!
//! Each case lays out a tiny on-disk store (the `_instance` sidecar +
//! per-dashboard artifacts copied from `tests/fixtures/<case>/`), runs the
//! real `build_report` entry point, and diffs the written `REPORT.md`
//! against the checked-in `expected.md`. Set `UPDATE_GOLDEN=1` to refresh
//! the goldens after an intentional change.
//!
//! Cases (per the stage contract):
//! - `overview_single`  — overview-only, one dashboard.
//! - `full_multi`       — full / no-data, multiple dashboards, template SQL.
//! - `full_data`        — full + data over a SQL datasource (executed SQL
//!   present) AND a non-SQL datasource (executed SQL absent, NOT faked).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use ncrawler_report_grafana::{build_report, Mode, ReportOptions, REPORT_FILENAME};
use ncrawler_spi::Cancel;

/// A never-cancelled handle for the synchronous build path.
struct NeverCancel;
impl Cancel for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn cancelled<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(std::future::pending())
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Lay out `<tmp>/sidecar/instance.json` + `<tmp>/<uid>/artifact.json` from
/// the case's fixture files, returning `(sidecar_dir, dashboard_dirs)`.
fn lay_out(case: &str, dashboards: &[&str]) -> (PathBuf, Vec<PathBuf>) {
    let src = fixtures().join(case);
    let tmp = std::env::temp_dir().join(format!(
        "ncrawler-grafana-golden-{case}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let sidecar_dir = tmp.join("sidecar");
    std::fs::create_dir_all(&sidecar_dir).unwrap();
    std::fs::copy(src.join("instance.json"), sidecar_dir.join("instance.json")).unwrap();

    let mut dirs = Vec::new();
    for uid in dashboards {
        let d = tmp.join(uid);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::copy(src.join(format!("{uid}.json")), d.join("artifact.json")).unwrap();
        dirs.push(d);
    }
    (sidecar_dir, dirs)
}

fn check(
    case: &str,
    dashboards: &[&str],
    scope: &str,
    inventory_total: usize,
    on_disk_total: usize,
    opts: ReportOptions,
) {
    let (sidecar_dir, dirs) = lay_out(case, dashboards);
    let out = build_report(
        &sidecar_dir,
        &dirs,
        scope,
        inventory_total,
        on_disk_total,
        &opts,
        &NeverCancel,
    )
    .expect("build_report");
    assert_eq!(out.files, vec![PathBuf::from(REPORT_FILENAME)]);

    let produced = std::fs::read_to_string(sidecar_dir.join(REPORT_FILENAME)).unwrap();
    let golden = fixtures().join(case).join("expected.md");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&golden, &produced).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&golden)
        .unwrap_or_else(|_| panic!("missing golden {} (run UPDATE_GOLDEN=1)", golden.display()));
    assert_eq!(produced, expected, "case `{case}` diverged from golden");
}

#[test]
fn overview_single_dashboard() {
    check(
        "overview_single",
        &["site-a"],
        "--uid site-a",
        3,
        1,
        ReportOptions {
            mode: Mode::Overview,
            data: false,
            window: "now-6h".to_owned(),
            redact: true,
        },
    );
}

#[test]
fn full_multi_dashboard_template_sql() {
    check(
        "full_multi",
        &["site-a", "nav"],
        "--all",
        3,
        2,
        ReportOptions {
            mode: Mode::Full,
            data: false,
            window: "now-6h".to_owned(),
            redact: true,
        },
    );
}

#[test]
fn full_with_data_sql_and_non_sql() {
    check(
        "full_data",
        &["sql-dash", "rubix-dash"],
        "--all",
        2,
        2,
        ReportOptions {
            mode: Mode::Full,
            data: true,
            window: "now-30d..now".to_owned(),
            redact: true,
        },
    );
}

/// The generic `Builder` seam (via `BuildCtx`) produces the same bytes as
/// the direct `build_report` entry point — proving the trait impl reads
/// the sidecar dir + `dashboard_dirs` + options correctly.
#[test]
fn builder_trait_path_matches_direct_entry() {
    use ncrawler_spi::{Artifact, BuildCtx, Builder};

    let (sidecar_dir, dirs) = lay_out("full_multi", &["site-a", "nav"]);
    let ctx = BuildCtx {
        artifact_dir: sidecar_dir.clone(),
        dashboard_dirs: dirs,
        options: serde_json::json!({
            "mode": "full",
            "data": false,
            "window": "now-6h",
            "redact": true,
            "scope": "--all",
            "inventory_total": 3,
            "on_disk_total": 2,
        }),
    };
    let placeholder = Artifact::new("grafana", "rd-esr.nube-iiot.com", chrono::Utc::now());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        ncrawler_report_grafana::GrafanaReportBuilder::new()
            .build(&placeholder, &ctx, &NeverCancel)
            .await
            .expect("trait build");
    });
    let produced = std::fs::read_to_string(sidecar_dir.join(REPORT_FILENAME)).unwrap();
    let golden =
        std::fs::read_to_string(fixtures().join("full_multi").join("expected.md")).unwrap();
    assert_eq!(produced, golden);
}

/// The SQL datasource panel exposes executed SQL; the non-SQL (Rubix)
/// panel does NOT — and the report must not fabricate one for it.
#[test]
fn non_sql_executed_query_is_never_faked() {
    let (sidecar_dir, dirs) = lay_out("full_data", &["sql-dash", "rubix-dash"]);
    build_report(
        &sidecar_dir,
        &dirs,
        "--all",
        2,
        2,
        &ReportOptions {
            mode: Mode::Full,
            data: true,
            window: "now-30d..now".to_owned(),
            redact: true,
        },
        &NeverCancel,
    )
    .unwrap();
    let md = std::fs::read_to_string(sidecar_dir.join(REPORT_FILENAME)).unwrap();
    // Exactly one executed-SQL block, and it belongs to the SQL panel.
    assert_eq!(md.matches("executed SQL").count(), 1);
    assert!(md.contains("FROM meters"));
    // The redactor masked the long-hex literal in both template + executed SQL.
    assert!(!md.contains("deadbeefdeadbeefdeadbeefdeadbeef"));
    // The secret-named variable value is masked.
    assert!(!md.contains("supersecretvalue"));
}
