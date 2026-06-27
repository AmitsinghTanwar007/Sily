//! Mapping a working directory to Claude's project-folder name.

/// Encode a working directory into Claude's project-folder name: every character
/// that isn't ASCII alphanumeric becomes `-` (so `/home/amitsinghtanwar` →
/// `-home-amitsinghtanwar`).
pub fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_cwd_matches_claude() {
        assert_eq!(encode_cwd("/home/amitsinghtanwar"), "-home-amitsinghtanwar");
        assert_eq!(
            encode_cwd("/home/x/Amit-docs/hyperswitch-prism"),
            "-home-x-Amit-docs-hyperswitch-prism"
        );
    }
}
