//! Shared dashboard selector (REPORT §2).
//!
//! One selector surface, parsed once, consumed by **both** the
//! `ncrawler scrape grafana` CLI and the (later) `report-grafana` builder
//! — so the report can resolve `--all/--uid/--name/--folder/--tag/--limit`
//! against the on-disk sidecar with exactly the same matching the scrape
//! used against the live `/api/search`.
//!
//! The selector lives in `ncrawler-grafana` (which already depends on
//! `ncrawler-core`) rather than in a builder, so the report builder can
//! depend on it without forming a dependency cycle.
//!
//! ## Dual `--all` (REPORT §2)
//!
//! `--all` does **not** mean the same thing in both phases:
//!
//! - At **scrape** time `--all` is the *live inventory* — every dashboard
//!   `/api/search` returns (the true 1000+ on `rd-esr`). Resolve with
//!   [`DashboardSelector::resolve_live`].
//! - At **report** time `--all` can only be *what is actually on disk* for
//!   the instance — whatever a prior scrape persisted. A `report --all`
//!   after `scrape --all --limit 50` reports **50**, not 1000+. Resolve
//!   with [`DashboardSelector::resolve_on_disk`], which is handed the
//!   sidecar's recorded full inventory so the report can print **both**
//!   counts in its header and `tracing::warn` when on-disk coverage is
//!   narrower than the instance inventory.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Safety upper bound on `--limit`. `--all` on this instance is 1000+
/// dashboards; a limit beyond this is almost certainly a typo and would
/// fan out an unbounded scrape, so it is rejected at parse time.
pub const MAX_LIMIT: u64 = 10_000;

/// One dashboard in the inventory, normalised from either a live
/// `/api/search` row or a sidecar `search` row (same wire shape:
/// `{ uid, title, folderTitle, tags }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardEntry {
    pub uid: String,
    pub title: String,
    /// `folderTitle` from `/api/search`; `None` for dashboards in no
    /// folder (the "General" / orphan case).
    pub folder: Option<String>,
    pub tags: Vec<String>,
}

impl DashboardEntry {
    /// Parse one `/api/search` (or sidecar `search`) row. Returns `None`
    /// for rows that are not dashboards (folders, or rows missing a uid).
    fn from_search_row(row: &Value) -> Option<Self> {
        // `/api/search` mixes dashboards and folders; only dashboards
        // carry a `uid` we can scrape/report on. Folder rows have
        // `type == "dash-folder"` and no usable dashboard uid here.
        if row.get("type").and_then(Value::as_str) == Some("dash-folder") {
            return None;
        }
        let uid = row.get("uid").and_then(Value::as_str)?.to_owned();
        let title = row
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let folder = row
            .get("folderTitle")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let tags = row
            .get("tags")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            uid,
            title,
            folder,
            tags,
        })
    }
}

/// Parse a `/api/search` array (live) or a sidecar `search` value into the
/// normalised inventory. A non-array value yields an empty inventory.
pub fn parse_inventory(search: &Value) -> Vec<DashboardEntry> {
    search
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(DashboardEntry::from_search_row)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse failures for the selector flag set.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SelectorError {
    /// No selection flag was given — refuse to silently scrape/report
    /// nothing (or, worse, everything).
    #[error("no dashboard selection given (use --all, --uid, --name, --folder, or --tag)")]
    EmptySelection,
    /// `--limit 0` or `--limit > MAX_LIMIT`.
    #[error("--limit must be between 1 and {MAX_LIMIT} (got {0})")]
    LimitOutOfRange(u64),
    /// `--limit` value did not parse as a non-negative integer.
    #[error("--limit `{0}` is not a non-negative integer")]
    LimitNotANumber(String),
    /// A value-taking flag (`--uid`/`--name`/…) appeared with no value.
    #[error("`{0}` given with no value")]
    MissingValue(String),
}

/// The parsed selection surface, shared by scrape and report (REPORT §2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashboardSelector {
    /// `--all`: operate over the whole inventory (see dual semantics in
    /// the module docs).
    pub all: bool,
    /// `--uid a,b,c` (comma list, repeatable) flattened + de-duplicated,
    /// original order preserved.
    pub uids: Vec<String>,
    /// `--name <substring>`: case-insensitive title substring match.
    pub name: Option<String>,
    /// `--folder <name>`: case-insensitive exact folder match.
    pub folder: Option<String>,
    /// `--tag <t>`: case-insensitive tag membership.
    pub tag: Option<String>,
    /// `--limit N`: cap the resolved count (1..=MAX_LIMIT).
    pub limit: Option<u64>,
}

impl DashboardSelector {
    /// Parse the selector out of a flat trailing-arg list (`--flag value`
    /// pairs plus the bare `--all`). Repeatable `--uid` occurrences are
    /// concatenated; each value may itself be a comma list. Validates the
    /// `--limit` safety bounds and rejects an empty selection.
    pub fn from_args(args: &[String]) -> Result<Self, SelectorError> {
        let mut sel = DashboardSelector::default();
        let mut i = 0;
        while i < args.len() {
            let arg = args[i].as_str();
            match arg {
                "--all" => {
                    sel.all = true;
                    i += 1;
                }
                "--uid" => {
                    let v = value_after(args, i, "--uid")?;
                    sel.push_uids(v);
                    i += 2;
                }
                "--name" => {
                    sel.name = Some(value_after(args, i, "--name")?.to_owned());
                    i += 2;
                }
                "--folder" => {
                    sel.folder = Some(value_after(args, i, "--folder")?.to_owned());
                    i += 2;
                }
                "--tag" => {
                    sel.tag = Some(value_after(args, i, "--tag")?.to_owned());
                    i += 2;
                }
                "--limit" => {
                    let v = value_after(args, i, "--limit")?;
                    sel.limit = Some(parse_limit(v)?);
                    i += 2;
                }
                // Unknown flags belong to the caller's wider flag set
                // (`--url`, `--mode`, `--from`, …); skip them so the
                // selector can be parsed out of the same trailing-arg list.
                _ => i += 1,
            }
        }
        sel.finish()
    }

    /// Append the (possibly comma-separated) `--uid` value, de-duplicating
    /// while preserving first-seen order.
    fn push_uids(&mut self, raw: &str) {
        for uid in raw.split(',') {
            let uid = uid.trim();
            if !uid.is_empty() && !self.uids.iter().any(|u| u == uid) {
                self.uids.push(uid.to_owned());
            }
        }
    }

    /// Reject an empty selection after parsing.
    fn finish(self) -> Result<Self, SelectorError> {
        if self.is_empty_selection() {
            return Err(SelectorError::EmptySelection);
        }
        Ok(self)
    }

    /// True when no selection criterion was supplied at all.
    fn is_empty_selection(&self) -> bool {
        !self.all
            && self.uids.is_empty()
            && self.name.is_none()
            && self.folder.is_none()
            && self.tag.is_none()
    }

    /// Apply the criteria to an inventory, returning matches in the
    /// deterministic order `(folder, title, uid)` (REPORT §6b stable
    /// ordering) with `--limit` applied last.
    fn filter<'a>(&self, inventory: &'a [DashboardEntry]) -> Vec<&'a DashboardEntry> {
        let mut matched: Vec<&DashboardEntry> =
            inventory.iter().filter(|e| self.matches(e)).collect();
        matched.sort_by(|a, b| {
            a.folder
                .cmp(&b.folder)
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| a.uid.cmp(&b.uid))
        });
        if let Some(limit) = self.limit {
            matched.truncate(limit as usize);
        }
        matched
    }

    /// Does one entry satisfy every supplied criterion? `--all` is an
    /// inclusive default: it widens the pool but never overrides an
    /// explicit `--uid`/`--name`/`--folder`/`--tag` narrowing.
    fn matches(&self, e: &DashboardEntry) -> bool {
        if !self.uids.is_empty() && !self.uids.iter().any(|u| u == &e.uid) {
            return false;
        }
        if let Some(name) = &self.name {
            if !e.title.to_lowercase().contains(&name.to_lowercase()) {
                return false;
            }
        }
        if let Some(folder) = &self.folder {
            match &e.folder {
                Some(f) if f.eq_ignore_ascii_case(folder) => {}
                _ => return false,
            }
        }
        if let Some(tag) = &self.tag {
            if !e.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                return false;
            }
        }
        true
    }

    /// **Scrape-time** resolution against the live `/api/search` inventory
    /// (REPORT §2). `--all` here is the true live inventory.
    pub fn resolve_live(&self, live_inventory: &[DashboardEntry]) -> Resolution {
        let selected = self
            .filter(live_inventory)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        Resolution {
            inventory_total: live_inventory.len(),
            on_disk_total: None,
            selected,
        }
    }

    /// **Report-time** resolution against the dashboards actually on disk
    /// (REPORT §2). `--all` here means only what was scraped.
    /// `sidecar_inventory_total` is the full instance inventory the sidecar
    /// recorded at scrape time; when on-disk coverage is narrower than that,
    /// a `tracing::warn` is emitted and the gap is recorded in the returned
    /// [`Resolution`] so the report header can show both counts.
    pub fn resolve_on_disk(
        &self,
        on_disk: &[DashboardEntry],
        sidecar_inventory_total: usize,
    ) -> Resolution {
        if on_disk.len() < sidecar_inventory_total {
            tracing::warn!(
                on_disk = on_disk.len(),
                instance_inventory = sidecar_inventory_total,
                "report coverage is narrower than the instance inventory: \
                 only {} of {} dashboards are on disk (run a wider `scrape` \
                 to close the gap)",
                on_disk.len(),
                sidecar_inventory_total,
            );
        }
        let selected = self
            .filter(on_disk)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        Resolution {
            inventory_total: sidecar_inventory_total,
            on_disk_total: Some(on_disk.len()),
            selected,
        }
    }
}

/// The outcome of resolving a [`DashboardSelector`] against an inventory.
///
/// Records the counts REPORT §2 requires in the report header: how many
/// dashboards were selected, the full instance inventory, and (at report
/// time) how many were actually on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// The selected dashboards, in stable `(folder, title, uid)` order.
    pub selected: Vec<DashboardEntry>,
    /// Full instance inventory size — the live `/api/search` count at
    /// scrape time, or the sidecar's recorded inventory at report time.
    pub inventory_total: usize,
    /// Report time only: how many dashboards are actually on disk (the
    /// pool `--all` draws from). `None` at scrape time.
    pub on_disk_total: Option<usize>,
}

impl Resolution {
    /// How many dashboards were selected.
    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// The selected uids, in resolution order.
    pub fn uids(&self) -> Vec<String> {
        self.selected.iter().map(|e| e.uid.clone()).collect()
    }

    /// True when on-disk coverage is narrower than the recorded instance
    /// inventory (report time). Always false at scrape time.
    pub fn coverage_is_narrow(&self) -> bool {
        matches!(self.on_disk_total, Some(n) if n < self.inventory_total)
    }

    /// A one-line scope description for the report header, e.g.
    /// `2 scraped of 1000 in instance inventory` (REPORT §4).
    pub fn header_scope(&self) -> String {
        format!(
            "{} selected of {} in instance inventory",
            self.selected_count(),
            self.inventory_total
        )
    }
}

/// Resolve the value following a value-taking flag at index `i`.
fn value_after<'a>(args: &'a [String], i: usize, flag: &str) -> Result<&'a str, SelectorError> {
    args.get(i + 1)
        .map(String::as_str)
        // Guard against `--uid --name foo` swallowing the next flag.
        .filter(|v| !v.starts_with("--"))
        .ok_or_else(|| SelectorError::MissingValue(flag.to_owned()))
}

/// Parse + bound-check a `--limit` value.
fn parse_limit(raw: &str) -> Result<u64, SelectorError> {
    let n: u64 = raw
        .parse()
        .map_err(|_| SelectorError::LimitNotANumber(raw.to_owned()))?;
    if n == 0 || n > MAX_LIMIT {
        return Err(SelectorError::LimitOutOfRange(n));
    }
    Ok(n)
}

/// The distinct uids the inventory exposes — handy for callers that need
/// to know what `--all` would draw from without resolving.
pub fn inventory_uids(inventory: &[DashboardEntry]) -> BTreeSet<String> {
    inventory.iter().map(|e| e.uid.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inventory() -> Vec<DashboardEntry> {
        parse_inventory(&json!([
            { "uid": "a1", "title": "Site Summary — Warehouse A", "folderTitle": "Sites", "tags": ["site", "prod"] },
            { "uid": "b2", "title": "Site Summary — Warehouse B", "folderTitle": "Sites", "tags": ["site"] },
            { "uid": "c3", "title": "Navigation", "tags": [] },
            { "uid": "d4", "title": "Energy MTD", "folderTitle": "Energy", "tags": ["energy", "prod"] },
            // A folder row + a uid-less row must be ignored.
            { "uid": "fdr", "title": "Sites", "type": "dash-folder" },
            { "title": "no uid here" }
        ]))
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_inventory_skips_folders_and_uidless_rows() {
        let inv = inventory();
        assert_eq!(inv.len(), 4);
        assert!(inv.iter().all(|e| e.uid != "fdr"));
        let nav = inv.iter().find(|e| e.uid == "c3").unwrap();
        assert_eq!(nav.folder, None);
        assert!(nav.tags.is_empty());
    }

    #[test]
    fn empty_selection_is_rejected() {
        assert_eq!(
            DashboardSelector::from_args(&args(&["--url", "https://x"])),
            Err(SelectorError::EmptySelection)
        );
    }

    #[test]
    fn all_selects_everything() {
        let sel = DashboardSelector::from_args(&args(&["--all"])).unwrap();
        let r = sel.resolve_live(&inventory());
        assert_eq!(r.selected_count(), 4);
        assert_eq!(r.inventory_total, 4);
        assert_eq!(r.on_disk_total, None);
    }

    #[test]
    fn single_uid_is_the_singleton_case_of_the_comma_list() {
        let sel = DashboardSelector::from_args(&args(&["--uid", "a1"])).unwrap();
        assert_eq!(sel.uids, vec!["a1"]);
        let r = sel.resolve_live(&inventory());
        assert_eq!(r.uids(), vec!["a1"]);
    }

    #[test]
    fn uid_comma_list_and_repeatable_dedup_and_preserve_order() {
        let sel =
            DashboardSelector::from_args(&args(&["--uid", "a1,b2", "--uid", "a1,d4"])).unwrap();
        assert_eq!(sel.uids, vec!["a1", "b2", "d4"]);
        let r = sel.resolve_live(&inventory());
        // Resolution is sorted by (folder, title, uid): Energy/d4, Sites/a1, Sites/b2.
        assert_eq!(r.uids(), vec!["d4", "a1", "b2"]);
    }

    #[test]
    fn uid_with_whitespace_is_trimmed() {
        let sel = DashboardSelector::from_args(&args(&["--uid", " a1 , b2 "])).unwrap();
        assert_eq!(sel.uids, vec!["a1", "b2"]);
    }

    #[test]
    fn name_is_case_insensitive_substring() {
        let sel = DashboardSelector::from_args(&args(&["--name", "warehouse"])).unwrap();
        let r = sel.resolve_live(&inventory());
        assert_eq!(r.uids(), vec!["a1", "b2"]);
    }

    #[test]
    fn folder_is_case_insensitive_exact() {
        let sel = DashboardSelector::from_args(&args(&["--folder", "sites"])).unwrap();
        let r = sel.resolve_live(&inventory());
        assert_eq!(r.uids(), vec!["a1", "b2"]);
    }

    #[test]
    fn tag_is_case_insensitive_membership() {
        let sel = DashboardSelector::from_args(&args(&["--tag", "PROD"])).unwrap();
        let r = sel.resolve_live(&inventory());
        // d4 (Energy) sorts before a1 (Sites).
        assert_eq!(r.uids(), vec!["d4", "a1"]);
    }

    #[test]
    fn criteria_combine_as_and() {
        let sel =
            DashboardSelector::from_args(&args(&["--folder", "Sites", "--tag", "prod"])).unwrap();
        let r = sel.resolve_live(&inventory());
        assert_eq!(r.uids(), vec!["a1"]);
    }

    #[test]
    fn all_with_a_narrowing_filter_still_narrows() {
        let sel = DashboardSelector::from_args(&args(&["--all", "--folder", "Energy"])).unwrap();
        let r = sel.resolve_live(&inventory());
        assert_eq!(r.uids(), vec!["d4"]);
    }

    #[test]
    fn limit_caps_after_stable_sort() {
        let sel = DashboardSelector::from_args(&args(&["--all", "--limit", "2"])).unwrap();
        let r = sel.resolve_live(&inventory());
        // First two in (folder, title, uid) order: Energy/d4, then no-folder
        // sorts as None < Some, so Navigation/c3 comes first overall.
        assert_eq!(r.uids(), vec!["c3", "d4"]);
    }

    #[test]
    fn limit_zero_is_rejected() {
        assert_eq!(
            DashboardSelector::from_args(&args(&["--all", "--limit", "0"])),
            Err(SelectorError::LimitOutOfRange(0))
        );
    }

    #[test]
    fn limit_over_bound_is_rejected() {
        assert_eq!(
            DashboardSelector::from_args(&args(&["--all", "--limit", "10001"])),
            Err(SelectorError::LimitOutOfRange(10_001))
        );
        // The bound itself is accepted.
        assert!(DashboardSelector::from_args(&args(&["--all", "--limit", "10000"])).is_ok());
    }

    #[test]
    fn limit_non_numeric_is_rejected() {
        assert_eq!(
            DashboardSelector::from_args(&args(&["--all", "--limit", "lots"])),
            Err(SelectorError::LimitNotANumber("lots".to_owned()))
        );
    }

    #[test]
    fn missing_value_is_rejected() {
        assert_eq!(
            DashboardSelector::from_args(&args(&["--uid"])),
            Err(SelectorError::MissingValue("--uid".to_owned()))
        );
        // A flag immediately following is treated as a missing value, not
        // swallowed.
        assert_eq!(
            DashboardSelector::from_args(&args(&["--name", "--all"])),
            Err(SelectorError::MissingValue("--name".to_owned()))
        );
    }

    #[test]
    fn report_time_all_is_only_what_is_on_disk() {
        // Scraped 2 of a 1000-dashboard instance.
        let on_disk = vec![inventory()[0].clone(), inventory()[1].clone()];
        let sel = DashboardSelector::from_args(&args(&["--all"])).unwrap();
        let r = sel.resolve_on_disk(&on_disk, 1000);
        assert_eq!(r.selected_count(), 2);
        assert_eq!(r.inventory_total, 1000);
        assert_eq!(r.on_disk_total, Some(2));
        assert!(r.coverage_is_narrow());
        assert_eq!(r.header_scope(), "2 selected of 1000 in instance inventory");
    }

    #[test]
    fn report_time_full_coverage_is_not_narrow() {
        let on_disk = inventory();
        let sel = DashboardSelector::from_args(&args(&["--all"])).unwrap();
        let r = sel.resolve_on_disk(&on_disk, on_disk.len());
        assert_eq!(r.on_disk_total, Some(4));
        assert!(!r.coverage_is_narrow());
    }

    #[test]
    fn live_vs_on_disk_divergence_for_same_selector() {
        // The SAME selector resolves differently against the live inventory
        // vs the narrower on-disk set — the dual-`--all` contract (REPORT §2).
        let live = inventory();
        let on_disk = vec![inventory()[0].clone()];
        let sel = DashboardSelector::from_args(&args(&["--all"])).unwrap();

        let live_res = sel.resolve_live(&live);
        let disk_res = sel.resolve_on_disk(&on_disk, live.len());

        assert_eq!(live_res.selected_count(), 4);
        assert_eq!(disk_res.selected_count(), 1);
        assert!(!live_res.coverage_is_narrow());
        assert!(disk_res.coverage_is_narrow());
    }

    #[test]
    fn no_match_resolves_empty() {
        let sel = DashboardSelector::from_args(&args(&["--name", "nonesuch"])).unwrap();
        assert_eq!(sel.resolve_live(&inventory()).selected_count(), 0);
    }
}
