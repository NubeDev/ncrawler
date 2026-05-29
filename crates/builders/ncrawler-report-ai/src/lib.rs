//! ncrawler — AI report builder.
//!
//! Composes the deterministic Markdown builder, a `starter-skills`
//! skill selection, and a `starter-ai` `provider-claude` run into a
//! single [`Builder`]. The pipeline (see [`pipeline`]) renders the
//! artifact to Markdown (the user prompt), resolves a skill (the system
//! prompt), then streams `Event`s from `ClaudeRunner`, persisting the
//! assistant text to `build-report-ai.md` and the event log to
//! `build-report-ai.jsonl` next to the artifact.
//!
//! ## Wiring
//!
//! [`AiReportBuilder::with_defaults`] is the production constructor: it
//! pulls the Claude runner out of [`Registry::with_defaults`] (only
//! `provider-claude` is compiled in) and builds a [`SkillRegistry`]
//! from a skills directory. [`AiReportBuilder::new`] takes the runner
//! and a [`SkillResolver`] directly, so tests inject a scripted runner
//! and a mock resolver with no real `claude` binary or bundles.
//!
//! ## Secrets
//!
//! Provider secrets are resolved through [`starter_ai::api_key_for`] and
//! placed only in the runner *input*. They never appear in a tracing
//! field, an `Event`, or the persisted logs (SCOPE: security).

#![forbid(unsafe_code)]

mod pipeline;
mod skills;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use ncrawler_spi::{Artifact, BuildCtx, BuildError, BuildOutput, Builder, Cancel};
use starter_ai::Registry;
use starter_skills::{InMemoryApprovalStore, SkillRegistry};
use starter_spi::ai::{AiRunner, Provider};

pub use pipeline::{run_pipeline, PipelineOutput, EVENT_LOG_FILENAME, REPORT_AI_FILENAME};
pub use skills::{RegistrySkillResolver, ResolvedSkill, SkillResolver};

/// The AI report [`Builder`].
pub struct AiReportBuilder {
    runner: Arc<dyn AiRunner>,
    resolver: Arc<dyn SkillResolver>,
    /// Default model hint; overridable per-build via `ctx.options.model`.
    model: Option<String>,
}

impl AiReportBuilder {
    /// Construct from an explicit runner + resolver. The seam used by
    /// unit tests (mock runner, mock resolver).
    pub fn new(runner: Arc<dyn AiRunner>, resolver: Arc<dyn SkillResolver>) -> Self {
        Self {
            runner,
            resolver,
            model: None,
        }
    }

    /// Set the default model hint passed to the runner.
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    /// Production constructor: Claude runner from
    /// [`Registry::with_defaults`] + a [`SkillRegistry`] loaded from
    /// `skills_dir`. Bundles are blake3 content-hashed on load; a
    /// `trust: quarantined` (or tampered) bundle simply won't be
    /// offered to the selector.
    pub async fn with_defaults(skills_dir: impl Into<PathBuf>) -> Result<Self, BuildError> {
        let registry = Registry::with_defaults();
        let runner = registry.get(&Provider::Claude).ok_or_else(|| {
            BuildError::Ai("provider-claude is not registered (feature disabled?)".to_owned())
        })?;
        let skill_registry = SkillRegistry::builder()
            .with_approval_store(InMemoryApprovalStore::new())
            .load_dir(skills_dir.into())
            .build()
            .await
            .map_err(|e| BuildError::Other(format!("loading skills: {e}")))?;
        let resolver = Arc::new(RegistrySkillResolver::new(skill_registry));
        Ok(Self {
            runner,
            resolver,
            model: None,
        })
    }
}

/// Read an optional `model` string override out of `ctx.options`.
fn model_override(ctx: &BuildCtx, default: &Option<String>) -> Option<String> {
    ctx.options
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .or_else(|| default.clone())
}

#[async_trait]
impl Builder for AiReportBuilder {
    fn name(&self) -> &str {
        "report-ai"
    }

    async fn build(
        &self,
        artifact: &Artifact,
        ctx: &BuildCtx,
        cancel: &dyn Cancel,
    ) -> Result<BuildOutput, BuildError> {
        let model = model_override(ctx, &self.model);
        let out = run_pipeline(
            &self.runner,
            self.resolver.as_ref(),
            artifact,
            &ctx.artifact_dir,
            model,
            cancel,
        )
        .await?;
        Ok(BuildOutput {
            files: out.files,
            summary: format!(
                "ai report via skill `{}` -> {REPORT_AI_FILENAME}",
                out.skill.skill_id
            ),
        })
    }
}
