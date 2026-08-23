use std::borrow::Cow;

use gpui::{App, BoxShadow, Hsla, point, px, rgb, rgba};
use gpui_component::Theme;

pub(crate) const FONT_SANS: &str = "Geist";
pub(crate) const FONT_MONO: &str = "Geist Mono";

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) background: Hsla,
    pub(crate) sidebar: Hsla,
    pub(crate) panel: Hsla,
    pub(crate) surface: Hsla,
    pub(crate) raised: Hsla,
    pub(crate) foreground: Hsla,
    pub(crate) muted: Hsla,
    pub(crate) subtle: Hsla,
    pub(crate) border: Hsla,
    pub(crate) border_strong: Hsla,
    pub(crate) hover: Hsla,
    pub(crate) selected: Hsla,
    pub(crate) accent: Hsla,
    pub(crate) accent_hover: Hsla,
    pub(crate) accent_active: Hsla,
    pub(crate) accent_soft: Hsla,
    pub(crate) good: Hsla,
    pub(crate) warn: Hsla,
    pub(crate) bad: Hsla,
    pub(crate) shadow: Hsla,
}

impl Palette {
    pub(crate) fn light() -> Self {
        Self {
            background: color(0xf7f7f7),
            sidebar: color(0xeeeeee),
            panel: color(0xfcfcfc),
            surface: color(0xf2f2f2),
            raised: color(0xffffff),
            foreground: color(0x1c1c1c),
            muted: color(0x696969),
            subtle: color(0x707070),
            border: color(0xe2e2e2),
            border_strong: color(0xcccccc),
            hover: color(0xe7e7e7),
            selected: color_alpha(0xc85c381a),
            accent: color(0xc85c38),
            accent_hover: color(0xb94f2e),
            accent_active: color(0xa94425),
            accent_soft: color_alpha(0xc85c3818),
            good: color(0x368363),
            warn: color(0xb57a2a),
            bad: color(0xbb4f4a),
            shadow: color_alpha(0x0000001a),
        }
    }

    pub(crate) fn dark() -> Self {
        Self {
            background: color(0x111111),
            sidebar: color(0x0d0d0d),
            panel: color(0x171717),
            surface: color(0x1e1e1e),
            raised: color(0x242424),
            foreground: color(0xf0f0f0),
            muted: color(0xa0a0a0),
            subtle: color(0x898989),
            border: color(0x292929),
            border_strong: color(0x3a3a3a),
            hover: color(0x222222),
            selected: color_alpha(0xd8785229),
            accent: color(0xd87852),
            accent_hover: color(0xe38460),
            accent_active: color(0xbc5d3e),
            accent_soft: color_alpha(0xd8785224),
            good: color(0x61aa83),
            warn: color(0xd6a251),
            bad: color(0xd16d67),
            shadow: color_alpha(0x00000052),
        }
    }

    pub(crate) fn panel_shadow(self) -> Vec<BoxShadow> {
        vec![BoxShadow {
            color: self.shadow,
            offset: point(px(0.), px(10.)),
            blur_radius: px(30.),
            spread_radius: px(-18.),
        }]
    }
}

pub(crate) fn init(cx: &mut App) -> anyhow::Result<()> {
    cx.text_system().add_fonts(vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Variable.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono-Variable.ttf")),
    ])?;

    let theme = Theme::global_mut(cx);
    theme.font_family = FONT_SANS.into();
    theme.mono_font_family = FONT_MONO.into();
    theme.font_size = px(14.);
    theme.mono_font_size = px(12.);
    theme.radius = px(6.);
    theme.radius_lg = px(10.);
    theme.tile_radius = px(10.);
    theme.shadow = false;
    theme.tile_shadow = false;
    Ok(())
}

pub(crate) fn apply_component_theme(dark: bool, cx: &mut App) {
    let palette = if dark {
        Palette::dark()
    } else {
        Palette::light()
    };
    let theme = Theme::global_mut(cx);

    theme.colors.background = palette.background;
    theme.colors.foreground = palette.foreground;
    theme.colors.border = palette.border;
    theme.colors.input = palette.border_strong;
    theme.colors.primary = palette.accent;
    theme.colors.primary_hover = palette.accent_hover;
    theme.colors.primary_active = palette.accent_active;
    theme.colors.primary_foreground = color(0xffffff);
    theme.colors.secondary = palette.surface;
    theme.colors.secondary_hover = palette.hover;
    theme.colors.secondary_active = palette.selected;
    theme.colors.secondary_foreground = palette.foreground;
    theme.colors.muted = palette.surface;
    theme.colors.muted_foreground = palette.muted;
    theme.colors.accent = palette.accent_soft;
    theme.colors.accent_foreground = palette.foreground;
    theme.colors.ring = palette.accent;
    theme.colors.selection = palette.selected;
    theme.colors.skeleton = palette.border;
    theme.colors.list_hover = palette.hover;
    theme.colors.list_active = palette.selected;
    theme.colors.list_active_border = palette.accent;
    theme.colors.sidebar = palette.sidebar;
    theme.colors.sidebar_border = palette.border;
    theme.colors.sidebar_foreground = palette.foreground;
    theme.colors.sidebar_accent = palette.selected;
    theme.colors.sidebar_accent_foreground = palette.foreground;
    theme.colors.sidebar_primary = palette.accent;
    theme.colors.sidebar_primary_foreground = color(0xffffff);
    theme.colors.table = palette.panel;
    theme.colors.table_head = palette.surface;
    theme.colors.table_head_foreground = palette.muted;
    theme.colors.table_hover = palette.hover;
    theme.colors.table_active = palette.selected;
    theme.colors.table_active_border = palette.accent;
    theme.colors.table_row_border = palette.border;
    theme.colors.popover = palette.raised;
    theme.colors.popover_foreground = palette.foreground;
    theme.colors.title_bar = palette.panel;
    theme.colors.title_bar_border = palette.border;
    theme.colors.success = palette.good;
    theme.colors.warning = palette.warn;
    theme.colors.danger = palette.bad;
    theme.colors.link = palette.accent;
    theme.colors.link_hover = palette.accent_hover;
}

pub(crate) fn color(value: u32) -> Hsla {
    rgb(value).into()
}

fn color_alpha(value: u32) -> Hsla {
    rgba(value).into()
}

#[cfg(test)]
mod tests {
    use super::Palette;

    #[test]
    fn interaction_colors_distinguish_default_hover_and_pressed_states() {
        for palette in [Palette::light(), Palette::dark()] {
            assert_ne!(palette.accent, palette.accent_hover);
            assert_ne!(palette.accent_hover, palette.accent_active);
            assert_ne!(palette.hover, palette.selected);
        }
    }

    #[test]
    fn structural_palette_colors_are_monochrome() {
        for palette in [Palette::light(), Palette::dark()] {
            for color in [
                palette.background,
                palette.sidebar,
                palette.panel,
                palette.surface,
                palette.raised,
                palette.foreground,
                palette.muted,
                palette.subtle,
                palette.border,
                palette.border_strong,
                palette.hover,
                palette.shadow,
            ] {
                assert_eq!(color.s, 0.0);
            }
        }
    }
}
