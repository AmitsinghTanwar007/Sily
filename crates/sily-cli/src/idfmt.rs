//! Compact, human-friendly id labels for list/tree/graph displays.
//!
//! Most providers use long opaque ids. Showing the first 8 chars works for UUIDs
//! often enough, but OpenCode ids share a long `ses_...` prefix, so a fixed
//! 8-char truncation produces collisions. These helpers keep labels compact
//! while making siblings distinguishable.

use std::collections::HashMap;

/// First `n` chars of `id`, or the whole id if it's shorter.
pub fn compact_label(id: &str, n: usize) -> String {
    id.chars().take(n).collect()
}

/// Smallest unique visible label for each id, with a floor of `min_chars`.
pub fn unique_labels<'a, I>(ids: I, min_chars: usize) -> HashMap<String, String>
where
    I: IntoIterator<Item = &'a str>,
{
    let ids: Vec<&str> = ids.into_iter().collect();
    let mut out = HashMap::new();
    for id in &ids {
        let width0 = min_chars.min(id.chars().count());
        let max = id.chars().count();
        let label = (width0..=max)
            .map(|width| compact_label(id, width))
            .find(|candidate| {
                ids.iter()
                    .filter(|other| *other != id)
                    .all(|other| compact_label(other, candidate.chars().count()) != *candidate)
            })
            .unwrap_or_else(|| (*id).to_string());
        out.insert((*id).to_string(), label);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{compact_label, unique_labels};

    #[test]
    fn compact_label_truncates() {
        assert_eq!(compact_label("abcdefghijk", 8), "abcdefgh");
    }

    #[test]
    fn unique_labels_disambiguate_shared_prefixes() {
        let labels = unique_labels(
            ["ses_1ea01234aaaa", "ses_1ea05678bbbb", "ses_3769ccccdddd"],
            8,
        );
        assert_ne!(labels["ses_1ea01234aaaa"], labels["ses_1ea05678bbbb"]);
        assert_eq!(labels["ses_3769ccccdddd"], "ses_3769");
    }

    #[test]
    fn unique_labels_handle_prefix_ids() {
        let labels = unique_labels(["abcdefghi", "abcdefghij"], 8);
        assert_eq!(labels["abcdefghi"], "abcdefghi");
        assert_eq!(labels["abcdefghij"], "abcdefghij");
    }
}
