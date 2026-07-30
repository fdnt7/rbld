//! turning what a command has to say into styled terminal output

use std::{fmt::Display, sync::OnceLock};

use {
    anstyle::{Ansi256Color, AnsiColor, Style},
    termtree::{GlyphPalette, Tree},
};

/// the `error` label, as opposed to the message it introduces
pub const ERROR: Style = AnsiColor::Red.on_default().bold();
/// the `fatal` label, brighter than [`ERROR`] because nothing ran at all
pub const FATAL: Style = AnsiColor::BrightRed.on_default().bold();
/// the message an `error` label introduces
pub const HEADLINE: Style = Style::new().bold();
/// a directory, bold so it reads apart from the files under it
pub const DIR: Style = Style::new().bold();
/// an aside spelling out in prose what a colour already says
pub const NOTE: Style = Style::new().dimmed();
/// the `step` label, naming the program about to be given the terminal
///
/// the same shape of line as [`ERROR`] and in blue where that is red, a
/// command being made of several programs in turn being no kind of trouble.
/// Blue also holds it apart from the green nh marks its own steps with, which
/// it sits directly above and would otherwise be taken for
pub const STEP: Style = AnsiColor::Blue.on_default().bold();
/// the `done` label, closing a command that got where it was going
///
/// the same shape of line as [`STEP`], being the last of them, and green as
/// the one colour worth spending on saying so; nh has stopped talking by the
/// time this is said, so there is nothing green left for it to be taken for
pub const DONE: Style = AnsiColor::Green.on_default().bold();
/// a file that is itself the reason a command refused to run, so it is marked
/// rather than merely listed
///
/// the background is a dark red rather than [`AnsiColor::Red`], which is bright
/// enough to swallow the text on it; `dimmed` is no help here, being an
/// intensity applied to the foreground
pub const DISALLOWED: Style =
    Style::new().bg_color(Some(anstyle::Color::Ansi256(Ansi256Color(52))));
/// tree scaffolding, dim so it recedes behind what it holds
const BRANCH: Style = Style::new().dimmed();

/// wraps `text` in `style`, closing it so the styling cannot bleed into
/// whatever follows; a default `Style` leaves `text` untouched
pub fn paint(style: Style, text: &str) -> String {
    format!("{}{text}{}", style.render(), style.render_reset())
}

/// a line saying what kind of line it is before it says anything else
///
/// `error`, `fatal`, `note`, `step`, `done`: every line rbld says in its own voice
/// opens with one of these, and the label is what the reader picks out first
/// and what tells them how much of their attention the rest of it is owed.
/// Only the label is styled, the message being left to whoever composed it,
/// which is how a headline comes through bold and a step's message plain.
pub fn labelled(style: Style, label: &str, message: impl Display) -> String {
    format!("{}: {message}", paint(style, label))
}

/// a tree waiting for what sits under `root`
///
/// the paths git reports are relative to the work tree, so it is the work tree
/// that roots a listing of them and the only line in it spelled out in full.
/// A trailing slash is left to the branches to draw, except where trimming it
/// would leave nothing to name at all.
pub fn rooted(root: &str) -> Tree<String> {
    let root = root.trim_end_matches('/');

    Tree::new(paint(DIR, if root.is_empty() { "/" } else { root })).with_glyphs(branches())
}

/// a file path, with the directories leading to it in bold
///
/// `base` styles the whole of it, so a caller can mark a file out without
/// giving up the distinction between where it sits and what it is called
pub fn file_path(base: Style, path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, file)) => format!(
            "{}{}",
            paint(base.bold(), &format!("{dir}/")),
            paint(base, file)
        ),
        None => paint(base, path),
    }
}

/// dims the branches so they recede and let the entries carry the tree; set on
/// the root alone, since termtree hands a node's glyphs down to every
/// descendant that does not override them
///
/// each piece is styled identically: termtree pairs `middle_item` with
/// `middle_skip` and `item_indent` with `skip_indent`, and asserts the two
/// halves of each pair are the same width
pub fn branches() -> GlyphPalette {
    // termtree wants `&'static str`, which borrowing a static gives without
    // leaking, however many times this ends up being called
    static GLYPHS: OnceLock<[String; 6]> = OnceLock::new();
    let [
        middle_item,
        last_item,
        item_indent,
        middle_skip,
        last_skip,
        skip_indent,
    ] = GLYPHS.get_or_init(|| {
        [
            "\u{251c}",
            "\u{2514}",
            "\u{2500}\u{2500} ",
            "\u{2502}",
            " ",
            "   ",
        ]
        .map(|glyph| paint(BRANCH, glyph))
    });

    GlyphPalette {
        middle_item,
        last_item,
        item_indent,
        middle_skip,
        last_skip,
        skip_indent,
    }
}
