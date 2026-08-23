use floem::peniko::{Brush, Color, Gradient};
use floem::text::Weight;

pub const LOGO_PNG: &[u8] = include_bytes!("../../../../resources/icons/otoa-input-192.png");
pub const MARK_SVG: &str = include_str!("../../../../resources/icons/otoa-mark.svg");
pub const WORDMARK_SVG: &str = include_str!("../../../../resources/icons/otoa-wordmark.svg");

#[allow(dead_code)]
pub mod color {
    use super::Color;

    pub const BG: Color = Color::rgb8(0xee, 0xf4, 0xfc);
    pub const SURFACE: Color = Color::rgb8(0xff, 0xff, 0xff);
    pub const LINE: Color = Color::rgb8(0xdb, 0xe6, 0xf5);
    pub const LINE_SOFT: Color = Color::rgb8(0xdf, 0xe6, 0xf0);
    pub const INK: Color = Color::rgb8(0x0f, 0x1a, 0x2e);
    pub const INK_SOFT: Color = Color::rgb8(0x5a, 0x66, 0x85);
    pub const BRAND: Color = Color::rgb8(0x2f, 0x7f, 0xe0);
    pub const BRAND_STRONG: Color = Color::rgb8(0x1c, 0x5c, 0xbb);
    pub const BRAND_2: Color = Color::rgb8(0x6f, 0xb0, 0xf0);
    pub const BRAND_TINT: Color = Color::rgb8(0xe6, 0xf1, 0xff);
    pub const CYAN: Color = Color::rgb8(0x46, 0xb6, 0xe6);
    pub const AMBER: Color = Color::rgb8(0xff, 0xb8, 0x4d);
    pub const NAVY: Color = Color::rgb8(0x0d, 0x1f, 0x3c);
    pub const NAVY_SOFT: Color = Color::rgb8(0x9f, 0xb3, 0xd6);
    pub const ERROR: Color = Color::rgb8(0xd9, 0x2d, 0x20);
    pub const ON_BRAND: Color = Color::rgb8(0xff, 0xff, 0xff);
    pub const RING: Color = Color::rgba8(0x2f, 0x7f, 0xe0, 0x38);

    // 旧設定画面が使っている名前。P2 で設定画面を作り直すまで残す。
    pub const IDLE: Color = NAVY_SOFT;
    pub const ACTIVE: Color = BRAND;
    pub const BUSY: Color = AMBER;
    pub const BORDER: Color = LINE;
    pub const TEXT: Color = INK;
    pub const TEXT_MUTED: Color = INK_SOFT;
}

pub fn grad_brand() -> Brush {
    Brush::Gradient(Gradient::new_linear((0.0, 40.0), (40.0, 0.0)).with_stops([
        (0.0, Color::rgb8(0x3d, 0x86, 0xd9)),
        (1.0, color::BRAND_STRONG),
    ]))
}

#[allow(dead_code)]
pub mod text {
    use super::Weight;

    pub const TITLE: f32 = 20.0;
    pub const SECTION: f32 = 16.0;
    pub const BODY: f32 = 14.0;
    pub const BODY_SOFT: f32 = 14.0;
    pub const TRANSCRIPT: f32 = 15.0;
    pub const CAPTION: f32 = 12.5;
    pub const MICRO: f32 = 11.0;

    pub const TITLE_WEIGHT: Weight = Weight::BLACK;
    pub const SECTION_WEIGHT: Weight = Weight(800);
    pub const BODY_WEIGHT: Weight = Weight(600);
    pub const BODY_SOFT_WEIGHT: Weight = Weight::MEDIUM;
    pub const TRANSCRIPT_WEIGHT: Weight = Weight(700);
    pub const CAPTION_WEIGHT: Weight = Weight::MEDIUM;
    pub const MICRO_WEIGHT: Weight = Weight::BLACK;
}

pub mod space {
    pub const XS: f64 = 4.0;
    pub const SM: f64 = 8.0;
    pub const MD: f64 = 12.0;
    pub const LG: f64 = 16.0;
    pub const XL: f64 = 24.0;
    pub const XXL: f64 = 32.0;
}

pub mod radius {
    pub const SM: f64 = 10.0;
    pub const BAR: f64 = 16.0;
    pub const CARD: f64 = 24.0;
    pub const LG: f64 = 32.0;
    pub const PILL: f64 = 999.0;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub blur: f64,
    pub h_offset: f64,
    pub v_offset: f64,
    pub spread: f64,
    pub color: Color,
}

pub mod shadow {
    use super::{Color, Shadow};

    pub const E1: Shadow = Shadow {
        blur: 24.0,
        h_offset: 0.0,
        v_offset: 10.0,
        spread: 0.0,
        color: Color::rgba8(0x0d, 0x1f, 0x3c, 0x3d),
    };
    pub const E3: Shadow = Shadow {
        blur: 48.0,
        h_offset: 0.0,
        v_offset: 24.0,
        spread: 0.0,
        color: Color::rgba8(0x0d, 0x1f, 0x3c, 0x6b),
    };
    pub const BRAND_GLOW: Shadow = Shadow {
        blur: 22.0,
        h_offset: 0.0,
        v_offset: 10.0,
        spread: 0.0,
        color: Color::rgba8(0x2f, 0x7f, 0xe0, 0x73),
    };
}

pub mod motion {
    pub const RING_PULSE: f64 = 2.6;
    pub const EQ_IDLE: f64 = 1.15;
    pub const CARET: f64 = 1.1;
    pub const APPEAR: f64 = 0.16;
    pub const HOVER: f64 = 0.2;
}

/// UI 全体のフォントファミリ。OS ごとに実在する最良のものを先頭に置く。
pub fn font_family() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Hiragino Sans, Hiragino Kaku Gothic ProN, Noto Sans CJK JP, sans-serif"
    }
    #[cfg(target_os = "windows")]
    {
        "Yu Gothic UI, Meiryo, Noto Sans CJK JP, sans-serif"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "Noto Sans CJK JP, Noto Sans JP, IPAexGothic, sans-serif"
    }
}
