//! Splitting `Item`s into embedding-sized chunks.
//!
//! Chunking is deterministic: the same `Item` always yields the same chunks,
//! so re-embedding a re-scraped (but unchanged) item produces identical
//! records and the upsert is a no-op in content terms.

use ncrawler_spi::{Artifact, Item};

/// A single embedding-sized slice of an item's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// The owning `Item.id` (stable across re-scrapes).
    pub item_id: String,
    /// 0-based index of this chunk within the item.
    pub seq: usize,
    pub text: String,
}

/// Chunking parameters. Sizes are in `char`s (not bytes) so multi-byte text
/// never splits mid-codepoint.
#[derive(Debug, Clone, Copy)]
pub struct ChunkConfig {
    /// Maximum characters per chunk.
    pub max_chars: usize,
    /// Characters of overlap carried from the end of one chunk into the next.
    pub overlap: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        // ~512 tokens for typical English ≈ 2000 chars; modest overlap keeps
        // context across boundaries without bloating the store.
        Self {
            max_chars: 2000,
            overlap: 200,
        }
    }
}

impl ChunkConfig {
    fn step(&self) -> usize {
        // Guard against pathological configs that would never advance.
        self.max_chars.saturating_sub(self.overlap).max(1)
    }
}

/// Chunk one item. An item with empty text yields a single empty chunk so it
/// still has a presence (and a stable key) in the store.
pub fn chunk_item(item: &Item, cfg: ChunkConfig) -> Vec<Chunk> {
    let body = render_item(item);
    let chars: Vec<char> = body.chars().collect();
    if chars.is_empty() {
        return vec![Chunk {
            item_id: item.id.clone(),
            seq: 0,
            text: String::new(),
        }];
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut seq = 0usize;
    while start < chars.len() {
        let end = (start + cfg.max_chars).min(chars.len());
        let text: String = chars[start..end].iter().collect();
        chunks.push(Chunk {
            item_id: item.id.clone(),
            seq,
            text,
        });
        seq += 1;
        if end == chars.len() {
            break;
        }
        start += cfg.step();
    }
    chunks
}

/// Chunk every item in an artifact, in item order.
pub fn chunk_artifact(artifact: &Artifact, cfg: ChunkConfig) -> Vec<Chunk> {
    artifact
        .items
        .iter()
        .flat_map(|it| chunk_item(it, cfg))
        .collect()
}

/// Build the text we actually embed: title (if any) followed by the item body.
fn render_item(item: &Item) -> String {
    match &item.title {
        Some(t) if !t.is_empty() => format!("{t}\n\n{}", item.text),
        _ => item.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ncrawler_spi::ItemKind;

    fn item(id: &str, title: Option<&str>, text: &str) -> Item {
        Item {
            id: id.into(),
            kind: ItemKind::Panel,
            title: title.map(Into::into),
            text: text.into(),
            data: None,
            tags: vec![],
        }
    }

    #[test]
    fn short_item_is_one_chunk_with_title_prefixed() {
        let chunks = chunk_item(
            &item("p1", Some("CPU"), "usage high"),
            ChunkConfig::default(),
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].seq, 0);
        assert_eq!(chunks[0].item_id, "p1");
        assert!(chunks[0].text.starts_with("CPU"));
        assert!(chunks[0].text.contains("usage high"));
    }

    #[test]
    fn empty_item_still_yields_one_chunk() {
        let chunks = chunk_item(&item("p1", None, ""), ChunkConfig::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "");
    }

    #[test]
    fn long_item_splits_with_overlap_and_is_deterministic() {
        let body: String = "x".repeat(50);
        let cfg = ChunkConfig {
            max_chars: 20,
            overlap: 5,
        };
        let it = item("p1", None, &body);
        let a = chunk_item(&it, cfg);
        let b = chunk_item(&it, cfg);
        assert_eq!(a, b, "chunking must be deterministic");
        assert!(a.len() > 1);
        // seqs are contiguous from 0.
        for (i, c) in a.iter().enumerate() {
            assert_eq!(c.seq, i);
        }
        // step = max_chars - overlap = 15; chunk 1 starts at char 15.
        assert_eq!(a[0].text.len(), 20);
    }

    #[test]
    fn multibyte_text_never_panics() {
        let body: String = "é".repeat(40);
        let cfg = ChunkConfig {
            max_chars: 10,
            overlap: 2,
        };
        let chunks = chunk_item(&item("p1", None, &body), cfg);
        assert!(chunks.iter().all(|c| !c.text.is_empty()));
    }
}
