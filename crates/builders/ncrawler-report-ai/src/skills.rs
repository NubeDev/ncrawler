//! Skill resolution seam.
//!
//! The AI builder needs exactly one thing from `starter-skills`: given
//! an artifact's `source` + the union of its item `tags`, resolve the
//! single best-matching skill bundle and hand back its system prompt
//! (the skill *body*) plus the blake3 `content_hash` it was selected
//! with. That narrow need is captured by the [`SkillResolver`] trait so
//! the builder pipeline can be unit-tested against a mock without a
//! real `SkillRegistry` or on-disk bundles.
//!
//! [`RegistrySkillResolver`] is the production impl: it drives the real
//! `starter_skills::SkillRegistry` through the `SkillSelector` seam.
//! Quarantined bundles never reach the selector (the registry filters
//! them out by construction), so a tampered bundle simply fails to
//! resolve rather than running with an unverified system prompt.

use std::collections::BTreeMap;

use async_trait::async_trait;

use ncrawler_spi::BuildError;
use starter_flow_spi::node::{SlotMap, SlotValue};
use starter_flow_spi::skill::{SkillSelection, SkillSelector};
use starter_flow_spi::Principal;
use starter_skills::SkillRegistry;
use starter_spi::auth::Role;

/// A resolved skill: everything the pipeline needs to drive the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    /// Reverse-DNS skill id the selector chose.
    pub skill_id: String,
    /// The skill body, used verbatim as the runner's system prompt.
    /// `starter-skills` performs no templating — `{{x}}` is literal.
    pub system_prompt: String,
    /// blake3 content hash of the bundle at selection time. Logged so
    /// a run is reproducible; never used to fetch anything.
    pub content_hash: String,
}

/// Resolve at most one skill for an artifact's `source` + `tags`.
#[async_trait]
pub trait SkillResolver: Send + Sync {
    /// Returns `Ok(Some(_))` when a skill matched, `Ok(None)` when the
    /// registry had no candidate. Errors are reserved for genuine
    /// resolution failures (selector backend errors, missing body).
    async fn resolve(
        &self,
        source: &str,
        tags: &[String],
    ) -> Result<Option<ResolvedSkill>, BuildError>;
}

/// Build the selector input from an artifact's `source` + `tags`.
///
/// Slot names are stable so the deterministic [`KeywordSkillSelector`]
/// produces the same match for the same artifact across runs.
///
/// [`KeywordSkillSelector`]: starter_skills::KeywordSkillSelector
fn selector_input(source: &str, tags: &[String]) -> SlotMap {
    let mut map: SlotMap = BTreeMap::new();
    map.insert("source".to_owned(), SlotValue::String(source.to_owned()));
    map.insert("tags".to_owned(), SlotValue::String(tags.join(" ")));
    map
}

/// A read-only single-operator principal. `ncrawler` has no multi-tenant
/// auth model (SCOPE: non-goals); the selector only reads it.
fn operator_principal() -> Principal {
    Principal {
        subject: "ncrawler".to_owned(),
        role: Role::Reader,
        scopes: Vec::new(),
        tenant_id: None,
        teams: Vec::new(),
        extra: serde_json::Value::Null,
    }
}

/// Production [`SkillResolver`] backed by a real [`SkillRegistry`].
pub struct RegistrySkillResolver {
    registry: SkillRegistry,
}

impl RegistrySkillResolver {
    /// Wrap an already-built registry (bundles loaded + hash-verified).
    pub fn new(registry: SkillRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl SkillResolver for RegistrySkillResolver {
    async fn resolve(
        &self,
        source: &str,
        tags: &[String],
    ) -> Result<Option<ResolvedSkill>, BuildError> {
        let input = selector_input(source, tags);
        let principal = operator_principal();
        let selection = self
            .registry
            .select(&input, &principal)
            .await
            .map_err(|e| BuildError::Other(format!("skill selection failed: {e}")))?;
        let (skill_id, content_hash) = match selection {
            SkillSelection::None => return Ok(None),
            SkillSelection::Selected {
                skill_id,
                content_hash,
                ..
            } => (skill_id, content_hash),
            // `SkillSelection` is `#[non_exhaustive]`; a future variant
            // is treated as "no skill" rather than crashing the build.
            _ => return Ok(None),
        };
        // The selection carries the id + hash; the body lives on the
        // approved `Skill` the registry still holds.
        let skill = self.registry.get(&skill_id).ok_or_else(|| {
            BuildError::MissingSkill(format!("selector chose `{skill_id}` but it is not loaded"))
        })?;
        Ok(Some(ResolvedSkill {
            skill_id: skill_id.to_string(),
            system_prompt: skill.body.to_string(),
            content_hash,
        }))
    }
}
