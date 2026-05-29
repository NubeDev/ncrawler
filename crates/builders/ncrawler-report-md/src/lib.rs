//! ncrawler — deterministic Markdown report builder.
//!
//! Renders an [`Artifact`] to Markdown with zero network and zero AI:
//! a per-[`Item`] section (title, tags, fenced JSON for `data`) plus
//! image embeds for the [`Asset`]s whose `item_id` matches the item.
//!
//! Asset↔item linkage is by `item_id` ONLY — never by `label` (SCOPE:
//! Asset ↔ Item linkage). The output is byte-stable for a given
//! artifact so it can be diffed in PRs and fed to the AI builder.

use std::path::PathBuf;

use async_trait::async_trait;

use ncrawler_spi::{Artifact, Asset, BuildCtx, BuildError, BuildOutput, Builder, Cancel, Item};

/// Filename written into the artifact directory.
pub const REPORT_FILENAME: &str = "report.md";

/// The deterministic Markdown [`Builder`].
#[derive(Default)]
pub struct MarkdownBuilder;

impl MarkdownBuilder {
    /// Construct a builder.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Builder for MarkdownBuilder {
    fn name(&self) -> &str {
        "report-md"
    }

    async fn build(
        &self,
        artifact: &Artifact,
        ctx: &BuildCtx,
        cancel: &dyn Cancel,
    ) -> Result<BuildOutput, BuildError> {
        if cancel.is_cancelled() {
            return Err(BuildError::Cancelled);
        }
        let markdown = render(artifact);
        let rel = PathBuf::from(REPORT_FILENAME);
        let abs = ctx.artifact_dir.join(&rel);
        std::fs::write(&abs, markdown).map_err(|e| BuildError::Io(e.to_string()))?;
        Ok(BuildOutput {
            files: vec![rel],
            summary: format!(
                "rendered {} item(s) to {REPORT_FILENAME}",
                artifact.items.len()
            ),
        })
    }
}

/// Render `artifact` to Markdown. Pure and deterministic: same artifact
/// in, same bytes out — no clock, no filesystem, no ordering surprises.
pub fn render(artifact: &Artifact) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} — {}\n\n", artifact.source, artifact.target));
    out.push_str(&format!(
        "_fetched_at: {}_\n",
        artifact
            .fetched_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ));
    for item in &artifact.items {
        out.push('\n');
        render_item(&mut out, item, &artifact.assets);
    }
    out
}

fn render_item(out: &mut String, item: &Item, assets: &[Asset]) {
    let heading = item.title.as_deref().unwrap_or(&item.id);
    out.push_str(&format!("## {heading}\n\n"));
    out.push_str(&format!("- id: `{}`\n", item.id));
    if !item.tags.is_empty() {
        let tags = item
            .tags
            .iter()
            .map(|t| format!("`{t}`"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("- tags: {tags}\n"));
    }
    if let Some(data) = &item.data {
        let pretty = serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string());
        out.push_str("\n```json\n");
        out.push_str(&pretty);
        out.push_str("\n```\n");
    }
    // Image embeds: assets linked to THIS item by `item_id`, in artifact
    // order. String-matching on `label` is forbidden, so two assets that
    // share a label still attach to whichever item their `item_id` names.
    for asset in assets
        .iter()
        .filter(|a| a.item_id.as_deref() == Some(&item.id))
    {
        out.push_str(&format!("\n![{}]({})\n", asset.label, asset.path.display()));
    }
}
