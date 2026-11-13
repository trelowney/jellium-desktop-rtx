//! Text-subtitle style handed over from the web layer.
//!
//! The Jellyfin Enhanced server plugin lets each user style subtitles in
//! jellyfin-web (text colour, background, size, font, text effect, vertical
//! position). jellyfin-web styles its own HTML subtitle overlay with it, but
//! here mpv draws the subtitles, so the web layer forwards the user's choice
//! and this module turns it into mpv `sub-*` properties.
//!
//! The first override snapshots the properties it is about to touch, so the
//! look the user had before (mpv defaults or their own `mpv.conf`) is restored
//! exactly once the plugin is absent or its custom styles are turned off.
//!
//! Only unstyled text subtitles (SRT, WebVTT, ...) are affected: mpv ignores
//! these properties for ASS subtitles, and the vertical position is only sent
//! by the web layer while a plain-text track is selected.

use parking_lot::Mutex;
use std::ffi::CString;

/// Every property [`properties`] can produce; the baseline snapshot covers
/// exactly these.
pub const STYLE_PROPERTIES: &[&str] = &[
    "sub-color",
    "sub-back-color",
    "sub-outline-color",
    "sub-border-style",
    "sub-font",
    "sub-font-size",
    "sub-outline-size",
    "sub-shadow-offset",
    "sub-blur",
    "sub-pos",
];

/// Jellyfin Enhanced's "Normal" size preset (1.2vw), the fallback for an
/// unusable size.
const NORMAL_SIZE_VW: f64 = 1.2;
/// Window aspect assumed when the web layer could not measure it.
const DEFAULT_ASPECT: f64 = 16.0 / 9.0;
/// mpv's `sub-font-size` is in pixels of a window scaled to 720 rows high.
const MPV_SCALED_HEIGHT: f64 = 720.0;
/// libass treats the font size as the font's full ascender-to-descender
/// height, which is larger than the CSS em the browser sizes by. Typical
/// subtitle fonts (Arial 1.12, DejaVu Sans 1.16, Segoe UI 1.33) sit around
/// this; measured on DejaVu Sans, a size of 100 gives the 63 px cap height
/// that an 86 px CSS font has.
const EM_TO_LIBASS_SIZE: f64 = 1.15;
/// Jellyfin Enhanced anchors the subtitle bottom edge at a percentage of the
/// video height (default 95). mpv's `sub-pos` 100 leaves a bottom margin of
/// about 5 % (`sub-margin-y` 34 at 720), so the plugin's default lands on
/// mpv's default position.
const POSITION_OFFSET: f64 = 5.0;

/// Text effect presets offered by Jellyfin Enhanced, matched by name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextEffect {
    /// Soft shadow on a transparent background, nothing on a solid one.
    Auto,
    None,
    Shadow,
    Outline,
    OutlineShadow,
}

impl TextEffect {
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "none" => Self::None,
            "shadow" => Self::Shadow,
            "outline" => Self::Outline,
            "outline + shadow" => Self::OutlineShadow,
            _ => Self::Auto,
        }
    }
}

/// The user's subtitle style as Jellyfin Enhanced stores it.
#[derive(Clone, Debug, PartialEq)]
pub struct SubtitleStyle {
    /// `#RRGGBB`, `#RRGGBBAA` or `transparent`.
    pub text_color: String,
    /// `#RRGGBB`, `#RRGGBBAA` or `transparent`.
    pub back_color: String,
    /// Font size in CSS `vw` (% of the window width), as the plugin sets it;
    /// its "Normal" preset is 1.2.
    pub size_vw: f64,
    /// Window width / height, to turn `vw` into mpv's height-relative size.
    pub viewport_aspect: f64,
    /// CSS font-family list; empty or `inherit` keeps the baseline font.
    pub font_family: String,
    pub effect: TextEffect,
    /// Bottom edge in % of the video height, or `None` to keep the baseline
    /// position (image and ASS subtitles).
    pub vertical_position: Option<f64>,
}

/// RGBA in 0.0..=1.0, as mpv's `r/g/b/a` colour syntax takes it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

impl Rgba {
    const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    /// Parses the colour formats Jellyfin Enhanced stores; `None` when the
    /// value is something else.
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("transparent") {
            return Some(Self::TRANSPARENT);
        }
        let hex = value.strip_prefix('#')?;
        if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .ok()
                .map(|v| f64::from(v) / 255.0)
        };
        Some(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if hex.len() == 8 { byte(6)? } else { 1.0 },
        })
    }

    fn mpv(self) -> String {
        format!(
            "{}/{}/{}/{}",
            fmt(self.r),
            fmt(self.g),
            fmt(self.b),
            fmt(self.a)
        )
    }
}

/// Short decimal form for property values (`0.5`, `38`, `1.234`).
fn fmt(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

/// First family of a CSS font-family list, without quotes; `None` for the
/// "keep the current font" values.
fn first_font_family(family: &str) -> Option<String> {
    let first = family
        .split(',')
        .next()?
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim();
    if first.is_empty() || first.eq_ignore_ascii_case("inherit") {
        None
    } else {
        Some(first.to_owned())
    }
}

/// mpv `sub-font-size` that renders as large as `vw` does in the browser in a
/// window of the given width / height.
fn font_size(vw: f64, aspect: f64) -> f64 {
    let vw = if vw.is_finite() && vw > 0.0 {
        vw
    } else {
        NORMAL_SIZE_VW
    };
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        DEFAULT_ASPECT
    };
    // vw of the width = vw * aspect % of the height.
    vw / 100.0 * aspect * MPV_SCALED_HEIGHT * EM_TO_LIBASS_SIZE
}

/// mpv property assignments for `style`. Pure so it can be unit-tested;
/// [`apply`] does the I/O.
pub fn properties(style: &SubtitleStyle) -> Vec<(&'static str, String)> {
    let size = font_size(style.size_vw, style.viewport_aspect);

    let text = Rgba::parse(&style.text_color).unwrap_or(Rgba {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    });
    let back = Rgba::parse(&style.back_color).unwrap_or(Rgba::TRANSPARENT);
    let has_box = back.a > 0.0;

    let effect = match style.effect {
        TextEffect::Auto if has_box => TextEffect::None,
        TextEffect::Auto => TextEffect::Shadow,
        other => other,
    };
    // Outline widths follow the plugin's em-based strokes (0.08em outline);
    // the soft shadow is a thinner blurred outline, as mpv has no glow.
    let (outline, blur) = match effect {
        TextEffect::None | TextEffect::Auto => (0.0, 0.0),
        TextEffect::Shadow => (size * 0.05, 1.5),
        TextEffect::Outline | TextEffect::OutlineShadow => (size * 0.08, 0.0),
    };

    let mut out = vec![
        ("sub-color", text.mpv()),
        ("sub-outline-color", "0/0/0/1".to_owned()),
        ("sub-font-size", fmt(size)),
        ("sub-outline-size", fmt(outline)),
        ("sub-blur", fmt(blur)),
    ];
    if has_box {
        // background-box: sub-back-color fills the box and sub-shadow-offset
        // is its margin (the plugin pads the box by 0.2em).
        out.push(("sub-border-style", "background-box".to_owned()));
        out.push(("sub-back-color", back.mpv()));
        out.push(("sub-shadow-offset", fmt(size * 0.2)));
    } else {
        // outline-and-shadow: sub-back-color is the drop-shadow colour.
        out.push(("sub-border-style", "outline-and-shadow".to_owned()));
        let shadow = if effect == TextEffect::OutlineShadow {
            size * 0.12
        } else {
            0.0
        };
        out.push(("sub-back-color", "0/0/0/0.85".to_owned()));
        out.push(("sub-shadow-offset", fmt(shadow)));
    }
    if let Some(font) = first_font_family(&style.font_family) {
        out.push(("sub-font", font));
    }
    if let Some(pos) = style.vertical_position.filter(|p| p.is_finite()) {
        out.push(("sub-pos", fmt((pos + POSITION_OFFSET).clamp(0.0, 100.0))));
    }
    out
}

/// Property values from before the first override, restored by [`clear`].
static BASELINE: Mutex<Option<Vec<(&'static str, String)>>> = Mutex::new(None);

fn read_property(name: &str) -> Option<String> {
    let name = CString::new(name).ok()?;
    let p = unsafe { crate::api::jfn_mpv_get_property_string(name.as_ptr()) };
    if p.is_null() {
        return None;
    }
    let value = unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned();
    unsafe { crate::api::jfn_mpv_free_string(p) };
    Some(value)
}

fn write_property(name: &str, value: &str) {
    let (Ok(name), Ok(value)) = (CString::new(name), CString::new(value)) else {
        return;
    };
    unsafe { crate::api::jfn_mpv_set_property_string_async(name.as_ptr(), value.as_ptr()) };
}

/// Applies `style` on top of the baseline, snapshotting it first if needed.
pub fn apply(style: &SubtitleStyle) {
    let mut baseline = BASELINE.lock();
    if baseline.is_none() {
        let snapshot: Vec<_> = STYLE_PROPERTIES
            .iter()
            .filter_map(|&name| read_property(name).map(|v| (name, v)))
            .collect();
        if snapshot.len() != STYLE_PROPERTIES.len() {
            // Without a complete snapshot the user's own look could not be
            // restored later, so leave mpv untouched.
            tracing::warn!(
                "subtitle style: could not read the current mpv subtitle style, not applying"
            );
            return;
        }
        *baseline = Some(snapshot);
    }
    let props = properties(style);
    // Properties the style leaves out (font, position) go back to baseline,
    // so switching e.g. to a bitmap track drops a previous position override.
    if let Some(b) = baseline.as_ref() {
        for (name, value) in b {
            if !props.iter().any(|(n, _)| n == name) {
                write_property(name, value);
            }
        }
    }
    for (name, value) in &props {
        write_property(name, value);
    }
}

/// Restores the look from before the first [`apply`]; no-op if nothing was
/// overridden.
pub fn clear() {
    if let Some(baseline) = BASELINE.lock().take() {
        for (name, value) in &baseline {
            write_property(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> SubtitleStyle {
        SubtitleStyle {
            text_color: "#FFFFFFFF".into(),
            back_color: "#00000000".into(),
            size_vw: 1.2,
            viewport_aspect: 16.0 / 9.0,
            font_family: "inherit".into(),
            effect: TextEffect::Auto,
            vertical_position: Some(95.0),
        }
    }

    fn get<'a>(props: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
        props
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn parses_plugin_colors() {
        assert_eq!(
            Rgba::parse("#0000007B").map(Rgba::mpv).as_deref(),
            Some("0/0/0/0.482")
        );
        assert_eq!(
            Rgba::parse("#ffffffFF").map(Rgba::mpv).as_deref(),
            Some("1/1/1/1")
        );
        assert_eq!(
            Rgba::parse("#FFFF00").map(Rgba::mpv).as_deref(),
            Some("1/1/0/1")
        );
        assert_eq!(
            Rgba::parse("transparent").map(Rgba::mpv).as_deref(),
            Some("0/0/0/0")
        );
        assert_eq!(Rgba::parse("red"), None);
        assert_eq!(Rgba::parse("#12345"), None);
        assert_eq!(Rgba::parse("#GG0000"), None);
    }

    #[test]
    fn default_style() {
        let p = properties(&style());
        // 1.2vw in a 16:9 window: 2.13 % of the height, 17.664 at 720.
        assert_eq!(get(&p, "sub-font-size"), Some("17.664"));
        assert_eq!(get(&p, "sub-pos"), Some("100"));
        assert_eq!(get(&p, "sub-border-style"), Some("outline-and-shadow"));
        // Auto on a transparent background is the soft shadow.
        assert_eq!(get(&p, "sub-outline-size"), Some("0.883"));
        assert_eq!(get(&p, "sub-blur"), Some("1.5"));
        assert_eq!(get(&p, "sub-shadow-offset"), Some("0"));
        assert_eq!(get(&p, "sub-font"), None);
    }

    #[test]
    fn background_uses_a_box() {
        let s = SubtitleStyle {
            back_color: "#0000007B".into(),
            size_vw: 1.8,
            ..style()
        };
        let p = properties(&s);
        assert_eq!(get(&p, "sub-border-style"), Some("background-box"));
        assert_eq!(get(&p, "sub-back-color"), Some("0/0/0/0.482"));
        assert_eq!(get(&p, "sub-font-size"), Some("26.496"));
        assert_eq!(get(&p, "sub-shadow-offset"), Some("5.299"));
        // Auto on a solid background draws no outline.
        assert_eq!(get(&p, "sub-outline-size"), Some("0"));
    }

    #[test]
    fn size_follows_the_window_shape_like_vw() {
        let size = |vw, aspect| {
            let s = SubtitleStyle {
                size_vw: vw,
                viewport_aspect: aspect,
                ..style()
            };
            get(&properties(&s), "sub-font-size").map(str::to_owned)
        };
        // Wider window, same height: vw text grows with the width.
        assert_eq!(size(1.2, 21.0 / 9.0).as_deref(), Some("23.184"));
        assert_eq!(size(1.2, 4.0 / 3.0).as_deref(), Some("13.248"));
        assert_eq!(size(2.4, 16.0 / 9.0).as_deref(), Some("35.328"));
        // Unusable inputs fall back to Normal in 16:9 instead of NaN.
        assert_eq!(size(f64::NAN, 16.0 / 9.0).as_deref(), Some("17.664"));
        assert_eq!(size(-1.0, 16.0 / 9.0).as_deref(), Some("17.664"));
        assert_eq!(size(1.2, f64::NAN).as_deref(), Some("17.664"));
        assert_eq!(size(1.2, 0.0).as_deref(), Some("17.664"));
    }

    #[test]
    fn effects() {
        let with = |effect| properties(&SubtitleStyle { effect, ..style() });
        assert_eq!(get(&with(TextEffect::None), "sub-outline-size"), Some("0"));
        assert_eq!(
            get(&with(TextEffect::Outline), "sub-outline-size"),
            Some("1.413")
        );
        assert_eq!(get(&with(TextEffect::Outline), "sub-blur"), Some("0"));
        let os = with(TextEffect::OutlineShadow);
        assert_eq!(get(&os, "sub-shadow-offset"), Some("2.12"));
        assert_eq!(get(&os, "sub-back-color"), Some("0/0/0/0.85"));
        assert_eq!(
            TextEffect::from_name("Outline + Shadow"),
            TextEffect::OutlineShadow
        );
        assert_eq!(TextEffect::from_name("something new"), TextEffect::Auto);
    }

    #[test]
    fn font_and_position() {
        let s = SubtitleStyle {
            font_family: "\"Courier New\",Courier,monospace".into(),
            vertical_position: Some(85.0),
            ..style()
        };
        let p = properties(&s);
        assert_eq!(get(&p, "sub-font"), Some("Courier New"));
        assert_eq!(get(&p, "sub-pos"), Some("90"));
        let off = SubtitleStyle {
            vertical_position: None,
            ..style()
        };
        assert_eq!(get(&properties(&off), "sub-pos"), None);
        let high = SubtitleStyle {
            vertical_position: Some(140.0),
            ..style()
        };
        assert_eq!(get(&properties(&high), "sub-pos"), Some("100"));
    }

    #[test]
    fn only_known_properties_are_written() {
        for effect in [
            TextEffect::Auto,
            TextEffect::None,
            TextEffect::Shadow,
            TextEffect::Outline,
            TextEffect::OutlineShadow,
        ] {
            for back in ["transparent", "#000000FF"] {
                let s = SubtitleStyle {
                    effect,
                    back_color: back.into(),
                    font_family: "Arial".into(),
                    ..style()
                };
                for (name, _) in properties(&s) {
                    assert!(STYLE_PROPERTIES.contains(&name), "{name}");
                }
            }
        }
    }
}
