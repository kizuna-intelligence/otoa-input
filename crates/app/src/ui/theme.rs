use floem::peniko::Color;

pub const LOGO_PNG: &[u8] = include_bytes!("../../../../resources/icons/otoa-input-192.png");

#[allow(dead_code)]
pub mod color {
    use super::Color;

    pub const BG: Color = Color::rgb8(0xee, 0xf4, 0xfc); // --bg
    pub const SURFACE: Color = Color::rgb8(0xff, 0xff, 0xff); // --surface
    pub const BORDER: Color = Color::rgb8(0xdb, 0xe6, 0xf5); // --line
    pub const TEXT: Color = Color::rgb8(0x0f, 0x1a, 0x2e); // --ink
    pub const TEXT_MUTED: Color = Color::rgb8(0x5a, 0x66, 0x85); // --ink-soft
    pub const BRAND: Color = Color::rgb8(0x2f, 0x7f, 0xe0); // --brand
    pub const BRAND_STRONG: Color = Color::rgb8(0x1c, 0x5c, 0xbb); // --brand-strong
    pub const CYAN: Color = Color::rgb8(0x46, 0xb6, 0xe6); // --cyan
    pub const AMBER: Color = Color::rgb8(0xff, 0xb8, 0x4d); // --amber
    pub const NAVY: Color = Color::rgb8(0x0d, 0x1f, 0x3c); // --navy
    pub const ON_BRAND: Color = Color::rgb8(0xff, 0xff, 0xff); // --on-brand

    pub const IDLE: Color = Color::rgb8(0x9f, 0xb3, 0xd6); // --navy-soft
    pub const ACTIVE: Color = BRAND;
    pub const BUSY: Color = AMBER;
    pub const ERROR: Color = Color::rgb8(0xd9, 0x2d, 0x20);
}

pub mod space {
    pub const XS: f64 = 6.0;
    pub const SM: f64 = 10.0;
    pub const MD: f64 = 14.0;
    pub const LG: f64 = 24.0;
    pub const XL: f64 = 28.0;
}

pub mod text {
    pub const TITLE: f32 = 20.0;
    pub const SECTION: f32 = 15.0;
    pub const BODY: f32 = 14.0;
    pub const CAPTION: f32 = 12.0;
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
