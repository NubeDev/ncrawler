//! The scrape→build AI pipeline, decomposed into testable steps.
//!
//! 1. Render the artifact to Markdown via `ncrawler-report-md` — the
//!    deterministic document becomes the runner's *user* prompt.
//! 2. Resolve a skill from the [`SkillResolver`]; its body is the
//!    *system* prompt. No skill ⇒ [`BuildError::MissingSkill`].
//! 3. Run `provider-claude` through `starter-ai`, streaming `Event`s.
//!    Each event is appended to `build-report-ai.jsonl`; the assistant
//!    text is written to `build-report-ai.md`.
//!
//! Cancellation is propagated end-to-end via `&dyn Cancel`. Secrets are
//! resolved through [`starter_ai::api_key_for`] and only ever placed in
//! the runner *input* — never into a tracing field, an `Event`, or the
//! on-disk logs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ncrawler_spi::{Artifact, BuildError, Cancel};
use starter_spi::ai::{AiRunner, CliCfg, Event, EventKind, Provider, RunnerInput, SessionId};
use tokio::sync::mpsc;

use crate::skills::{ResolvedSkill, SkillResolver};

/// Assistant text output, written next to the artifact.
pub const REPORT_AI_FILENAME: &str = "build-report-ai.md";
/// Streamed `Event` log (one JSON object per line).
pub const EVENT_LOG_FILENAME: &str = "build-report-ai.jsonl";

/// Outcome of a successful pipeline run.
pub struct PipelineOutput {
    /// Files written, relative to the artifact directory.
    pub files: Vec<PathBuf>,
    /// The resolved skill that drove the run.
    pub skill: ResolvedSkill,
}

/// The deduplicated union of every item's tags, sorted for determinism.
fn artifact_tags(artifact: &Artifact) -> Vec<String> {
    let mut tags: Vec<String> = artifact
        .items
        .iter()
        .flat_map(|i| i.tags.iter().cloned())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Build the Claude CLI input. The API key is resolved through the
/// `starter-ai` secret helper (CLI Claude takes none, so this is
/// `None`) and would be threaded only into the input if present —
/// keeping it out of events and logs by construction.
fn build_input(prompt: String, skill: &ResolvedSkill, model: Option<String>) -> RunnerInput {
    // Routed for the redaction guarantee even though Claude-CLI auth is
    // managed by the binary itself and returns `None` here.
    let _api_key = starter_ai::api_key_for(None, &Provider::Claude);
    RunnerInput::Cli(CliCfg {
        prompt,
        system_prompt: Some(skill.system_prompt.clone()),
        model,
        ..Default::default()
    })
}

/// Drive the runner while draining its event stream concurrently.
///
/// Returns the assistant text and the serialized JSONL event lines.
/// The runner future owns the `Sender`; the channel closes when it
/// completes, ending the drain loop.
async fn run_streaming(
    runner: &Arc<dyn AiRunner>,
    input: RunnerInput,
    cancel: &dyn Cancel,
) -> Result<(String, Vec<String>), BuildError> {
    let (tx, mut rx) = mpsc::channel::<Event>(256);
    let session = SessionId::from("ncrawler-report-ai");
    let run_fut = runner.run(input, session, tx, cancel);

    let drain = async {
        let mut lines = Vec::new();
        let mut text = String::new();
        while let Some(ev) = rx.recv().await {
            if let EventKind::Text { content } = &ev.kind {
                text.push_str(content);
            }
            if let Ok(line) = serde_json::to_string(&ev) {
                lines.push(line);
            }
        }
        (text, lines)
    };

    let (run_res, (streamed_text, lines)) = tokio::join!(run_fut, drain);
    let result = run_res.map_err(|e| BuildError::Ai(e.to_string()))?;
    if let Some(err) = result.error {
        return Err(BuildError::Ai(err));
    }
    // Prefer the runner's aggregated text; fall back to the stream.
    let text = if result.text.is_empty() {
        streamed_text
    } else {
        result.text
    };
    Ok((text, lines))
}

/// Persist the assistant text and the event log next to the artifact.
fn persist(dir: &Path, text: &str, lines: &[String]) -> Result<Vec<PathBuf>, BuildError> {
    let md_rel = PathBuf::from(REPORT_AI_FILENAME);
    let log_rel = PathBuf::from(EVENT_LOG_FILENAME);
    std::fs::write(dir.join(&md_rel), text).map_err(|e| BuildError::Io(e.to_string()))?;
    let mut log = lines.join("\n");
    if !log.is_empty() {
        log.push('\n');
    }
    std::fs::write(dir.join(&log_rel), log).map_err(|e| BuildError::Io(e.to_string()))?;
    Ok(vec![md_rel, log_rel])
}

/// Run the full pipeline. See the module docs for the four steps.
pub async fn run_pipeline(
    runner: &Arc<dyn AiRunner>,
    resolver: &dyn SkillResolver,
    artifact: &Artifact,
    artifact_dir: &Path,
    model: Option<String>,
    cancel: &dyn Cancel,
) -> Result<PipelineOutput, BuildError> {
    if cancel.is_cancelled() {
        return Err(BuildError::Cancelled);
    }
    let prompt = ncrawler_report_md::render(artifact);
    let tags = artifact_tags(artifact);
    let skill = resolver
        .resolve(&artifact.source, &tags)
        .await?
        .ok_or_else(|| {
            BuildError::MissingSkill(format!(
                "no skill matched source `{}` with tags {:?}",
                artifact.source, tags
            ))
        })?;
    tracing::info!(
        skill_id = %skill.skill_id,
        content_hash = %skill.content_hash,
        "resolved skill for ai report"
    );
    let input = build_input(prompt, &skill, model);
    let (text, lines) = run_streaming(runner, input, cancel).await?;
    let files = persist(artifact_dir, &text, &lines)?;
    Ok(PipelineOutput { files, skill })
}
