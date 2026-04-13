use std::fmt;

#[derive(Debug)]
#[cfg_attr(test, derive(strum::EnumIter))]
pub enum Icon {
    // Log level prefixes
    GroupArrow,
    Checkmark,
    Error,
    Warning,
    Bullet,

    // Impact / result indicators
    ImpactUp,
    ImpactDown,
    ImpactNeutral,

    // Executor icons
    ExecutorValgrind,
    ExecutorWallTime,
    ExecutorMemory,

    // Box-drawing (used by rolling buffer)
    BoxTopLeft,
    BoxTopRight,
    BoxBottomLeft,
    BoxBottomRight,
    BoxHorizontal,
    BoxVertical,
    BoxTDown,
    BoxTRight,
    BoxTLeft,

    // Miscellaneous
    Separator,
    Ellipsis,
}

impl Icon {
    /// Return the icon as a `char`. Panics if the icon's string is not a single character
    /// (all current icons are single codepoints, so this is always safe).
    pub fn as_char(&self) -> char {
        self.to_string()
            .chars()
            .next()
            .expect("icon must be a single character")
    }
}

impl fmt::Display for Icon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ch = match self {
            Icon::GroupArrow => "\u{203a}",       // ›
            Icon::Checkmark => "\u{2713}",        // ✓
            Icon::Error => "\u{2717}",            // ✗
            Icon::Warning => "\u{26a0}",          // ⚠
            Icon::Bullet => "\u{00b7}",           // ·
            Icon::ImpactUp => "\u{2191}",         // ↑
            Icon::ImpactDown => "\u{2193}",       // ↓
            Icon::ImpactNeutral => "\u{25cf}",    // ●
            Icon::ExecutorValgrind => "\u{2699}", // ⚙
            Icon::ExecutorWallTime => "\u{23f1}", // ⏱
            Icon::ExecutorMemory => "\u{25a4}",   // ▤
            Icon::BoxTopLeft => "\u{256d}",       // ╭
            Icon::BoxTopRight => "\u{256e}",      // ╮
            Icon::BoxBottomLeft => "\u{2570}",    // ╰
            Icon::BoxBottomRight => "\u{256f}",   // ╯
            Icon::BoxHorizontal => "\u{2500}",    // ─
            Icon::BoxVertical => "\u{2502}",      // │
            Icon::BoxTDown => "\u{252c}",         // ┬
            Icon::BoxTRight => "\u{251c}",        // ├
            Icon::BoxTLeft => "\u{2524}",         // ┤
            Icon::Separator => "\u{2500}",        // ─
            Icon::Ellipsis => "\u{2026}",         // …
        };
        f.write_str(ch)
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;

    #[test]
    fn test_icon_rendering() {
        let rendered = Icon::iter()
            .map(|icon| format!("{icon:?}: {icon}"))
            .collect::<Vec<_>>()
            .join("\n");

        insta::assert_snapshot!(rendered);
    }
}
