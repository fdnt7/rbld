//! turning what a command has to say into styled terminal output

use std::sync::OnceLock;

use {
    anstyle::{AnsiColor, Style},
    termtree::GlyphPalette,
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
/// tree scaffolding, dim so it recedes behind what it holds
const BRANCH: Style = Style::new().dimmed();

/// wraps `text` in `style`, closing it so the styling cannot bleed into
/// whatever follows; a default `Style` leaves `text` untouched
pub fn paint(style: Style, text: &str) -> String {
    format!("{}{text}{}", style.render(), style.render_reset())
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
    ] = GLYPHS.get_or_init(|| ["├", "└", "── ", "│", " ", "   "].map(|glyph| paint(BRANCH, glyph)));

    GlyphPalette {
        middle_item,
        last_item,
        item_indent,
        middle_skip,
        last_skip,
        skip_indent,
    }
}
