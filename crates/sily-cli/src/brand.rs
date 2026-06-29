//! sily's terminal identity: the `>_ sily` mark and the no-argument banner.

use owo_colors::{OwoColorize, Stream::Stdout, Style};

/// The wordmark, used by the banner and the interactive splash.
pub const MARK: &str = ">_ sily";

/// The static banner shown when `sily` is run with no command.
pub fn banner() -> String {
    let v = env!("CARGO_PKG_VERSION");
    format!(
        "\n{} {}   {}\n{}\n\n  {}     {}\n  {}   {}\n",
        ">_".if_supports_color(Stdout, |t| t.style(Style::new().green().bold())),
        "sily".if_supports_color(Stdout, |t| t.bold()),
        format!("v{v}").if_supports_color(Stdout, |t| t.dimmed()),
        "AI session version control · Claude Code · Codex · OpenCode"
            .if_supports_color(Stdout, |t| t.dimmed()),
        "sily list".if_supports_color(Stdout, |t| t.style(Style::new().green().bold())),
        "browse your sessions".if_supports_color(Stdout, |t| t.dimmed()),
        "sily --help".if_supports_color(Stdout, |t| t.style(Style::new().green().bold())),
        "all commands".if_supports_color(Stdout, |t| t.dimmed()),
    )
}
