//! Pure helpers: URL normalisation, stable item ids, and HTML → `Item`
//! extraction. Kept network-free so they unit-test without a crawl.

use serde_json::json;
use url::Url;

use ncrawler_spi::{Item, ItemKind};

/// Normalise a URL for stable identity (SCOPE: spider id rule):
/// lowercase host, sorted query pairs, fragment dropped. Falls back to
/// the trimmed raw string when the URL does not parse.
pub fn normalise_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return raw.trim().to_owned();
    };
    url.set_fragment(None);
    // `url` already lowercases the host; sort the query pairs for a
    // canonical ordering so `?b=2&a=1` and `?a=1&b=2` hash identically.
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        pairs.sort();
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &pairs {
            serializer.append_pair(k, v);
        }
        url.set_query(Some(&serializer.finish()));
    }
    url.into()
}

/// Deterministic `page-{blake3(normalised_url)[..16]}` id.
pub fn page_id(raw_url: &str) -> String {
    let norm = normalise_url(raw_url);
    let hex = blake3::hash(norm.as_bytes()).to_hex();
    format!("page-{}", &hex[..16])
}

/// Build an [`Item::Page`] from a page's URL + HTML: readable text via
/// `dom_smoothie`, an optional Markdown rendering via `fast_html2md`
/// stored under `data.markdown`.
pub fn html_to_item(url: &str, html: &str) -> Item {
    let (title, text) = readable(url, html);
    // `fast_html2md` exposes its lib as `html2md`.
    let markdown = html2md::rewrite_html(html, false);
    Item {
        id: page_id(url),
        kind: ItemKind::Page,
        title: title.clone(),
        text,
        data: Some(json!({ "url": url, "markdown": markdown })),
        tags: Vec::new(),
    }
}

/// Readable `(title, text)` via `dom_smoothie`, degrading gracefully to
/// `(None, "")` when extraction fails (best-effort — a page that is not
/// an article still yields a valid item, just empty readable text).
fn readable(url: &str, html: &str) -> (Option<String>, String) {
    let doc_url = Url::parse(url).ok().map(|_| url);
    let Ok(mut rd) = dom_smoothie::Readability::new(html, doc_url, None) else {
        return (None, String::new());
    };
    match rd.parse() {
        Ok(article) => {
            let title = if article.title.is_empty() {
                None
            } else {
                Some(article.title.clone())
            };
            (title, article.text_content.to_string())
        }
        Err(_) => (None, String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_sorts_query_and_drops_fragment() {
        let a = normalise_url("https://Example.com/p?b=2&a=1#frag");
        let b = normalise_url("https://example.com/p?a=1&b=2");
        assert_eq!(a, b);
        assert!(!a.contains("frag"));
    }

    #[test]
    fn id_is_stable_and_prefixed() {
        let id1 = page_id("https://example.com/x?z=1&y=2");
        let id2 = page_id("https://example.com/x?y=2&z=1#anchor");
        assert_eq!(id1, id2);
        assert!(id1.starts_with("page-"));
        assert_eq!(id1.len(), "page-".len() + 16);
    }

    #[test]
    fn extracts_readable_text_and_markdown() {
        let html = "<html><head><title>Hi</title></head><body>\
            <article><h1>Heading</h1><p>Hello world paragraph body.</p></article>\
            </body></html>";
        let item = html_to_item("https://example.com/a", html);
        assert_eq!(item.kind, ItemKind::Page);
        assert_eq!(item.id, page_id("https://example.com/a"));
        let md = item.data.unwrap();
        assert!(md["markdown"].as_str().unwrap().contains("Heading"));
    }
}
