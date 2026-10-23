//! angel-niri (stacking.rs): the title bars of windows while stacking.
//!
//! A floating window that left its decorations to the compositor (xdg-decoration server side) gets
//! a pixel title bar like Windows 98's: its icon and title on the left in the shell's title font,
//! minimize, maximize and close on the right, all on the art-pixel grid. The active window's bar
//! is in the focus ring's active colour, the others' in a dim grey of it; the buttons take their
//! face from the bar, so a shell that recolours the ring recolours them too. Windows that draw
//! their own title bars get none.
//!
//! The icon is the shell's: `$XDG_RUNTIME_DIR/angel-niri/title-icons/<app id>.png`, drawn eight
//! art pixels square with nearest filtering; a window without one has its title start at the left.
//!
//! Around such a window the pointer also grabs its edges to resize it, `GRAB` px outside.
//!
//! The Mac look (`--title-style mac`, Golden Gate) has three round pixel buttons on the left —
//! close, minimize, maximize in red, yellow and green, grey on inactive windows — and the title
//! in the middle, without an icon, on a light grey bar.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use niri_config::Color;
use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::{EllipsizeMode, FontDescription};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::utils::{Logical, Point, Rectangle, Size, Transform};

use crate::render_helpers::texture::TextureBuffer;
use crate::utils::ResizeEdge;

/// One art pixel of the pixel skins, in logical pixels.
const ART: f64 = 2.;

/// Height of a window's title bar: twelve art pixels.
pub const TITLE_HEIGHT: f64 = 12. * ART;

/// How far outside its window the pointer grabs an edge to resize it.
pub const GRAB: f64 = 6.;

/// How far along an edge from a corner the pointer grabs both edges.
const CORNER: f64 = 16.;

/// How often a window without an icon looks for one again.
const ICON_RETRY: Duration = Duration::from_secs(2);

thread_local! {
    static FONT: RefCell<String> = const { RefCell::new(String::new()) };
    static MAC: Cell<bool> = const { Cell::new(false) };
}

/// Whether the title bars have the Mac look.
pub fn set_mac(mac: bool) {
    MAC.with(|cell| cell.set(mac));
}

fn mac() -> bool {
    MAC.with(Cell::get)
}

/// The font of the titles, "Family SIZE" (size in logical px); empty for the default.
pub fn set_font(font: String) {
    FONT.with(|cell| *cell.borrow_mut() = font);
}

/// A part of a titled window's frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePart {
    Title,
    Minimize,
    Maximize,
    Close,
    Edge(ResizeEdge),
}

/// The buttons of a title bar of `width`, right to left on the art-pixel grid: eight by seven art
/// pixels each, minimize and maximize side by side, close two art pixels apart.
fn buttons(width: f64) -> [(FramePart, Rectangle<f64, Logical>); 3] {
    if mac() {
        return mac_buttons();
    }
    let (w, h) = (8. * ART, 7. * ART);
    let y = 2. * ART;
    let right = (width / ART).floor() * ART;
    let close = right - 2. * ART - w;
    let maximize = close - 2. * ART - w;
    let minimize = maximize - w;
    let rect = |x: f64| Rectangle::new(Point::from((x, y)), Size::from((w, h)));
    [
        (FramePart::Minimize, rect(minimize)),
        (FramePart::Maximize, rect(maximize)),
        (FramePart::Close, rect(close)),
    ]
}

/// The Mac look's buttons, left to right: six art pixels round, two apart, four from the left.
fn mac_buttons() -> [(FramePart, Rectangle<f64, Logical>); 3] {
    let rect = |i: f64| {
        Rectangle::new(
            Point::from(((4. + 8. * i) * ART, 3. * ART)),
            Size::from((6. * ART, 6. * ART)),
        )
    };
    [
        (FramePart::Close, rect(0.)),
        (FramePart::Minimize, rect(1.)),
        (FramePart::Maximize, rect(2.)),
    ]
}

/// Which part of the frame of a titled tile of `size` is at `pos` (relative to the tile, the
/// title bar on top): its title bar and buttons, or an edge up to `GRAB` px outside. `None` over
/// the window itself and beyond.
pub fn frame_part(pos: Point<f64, Logical>, size: Size<f64, Logical>) -> Option<FramePart> {
    let (x, y, w, h) = (pos.x, pos.y, size.w, size.h);
    if x < -GRAB || y < -GRAB || x >= w + GRAB || y >= h + GRAB {
        return None;
    }

    let out_x = x < 0. || x >= w;
    let out_y = y < 0. || y >= h;
    if out_x || out_y {
        let mut edges = ResizeEdge::empty();
        if x < 0. || (out_y && x < CORNER) {
            edges |= ResizeEdge::LEFT;
        } else if x >= w || (out_y && x >= w - CORNER) {
            edges |= ResizeEdge::RIGHT;
        }
        if y < 0. || (out_x && y < CORNER) {
            edges |= ResizeEdge::TOP;
        } else if y >= h || (out_x && y >= h - CORNER) {
            edges |= ResizeEdge::BOTTOM;
        }
        return Some(FramePart::Edge(edges));
    }

    if y >= TITLE_HEIGHT {
        return None;
    }
    let part = buttons(w)
        .into_iter()
        .find(|(_, rect)| rect.contains(pos))
        .map_or(FramePart::Title, |(part, _)| part);
    Some(part)
}

/// What a title bar shows; drawn again when it changes.
#[derive(Debug, Clone, PartialEq)]
struct Look {
    title: String,
    app_id: String,
    width: i32,
    scale: u64,
    active: bool,
    color: [u8; 4],
    font: String,
    corners: u32,
    icon: bool,
    mac: bool,
}

/// One window's title bar, drawn once per look.
#[derive(Debug, Default)]
pub struct TitleBar {
    look: Option<Look>,
    buffer: Option<TextureBuffer<GlesTexture>>,
    /// When it last looked for an icon and found none.
    no_icon_since: Option<Instant>,
}

impl TitleBar {
    /// The bar of `width` logical px for a window, drawn if its look changed; `color` is the focus
    /// ring's active colour.
    #[allow(clippy::too_many_arguments)]
    pub fn texture(
        &mut self,
        renderer: &mut GlesRenderer,
        title: &str,
        app_id: &str,
        width: f64,
        scale: f64,
        active: bool,
        color: Color,
    ) -> Option<TextureBuffer<GlesTexture>> {
        let icon_path = icon_path(app_id);
        let retry = self
            .no_icon_since
            .is_none_or(|since| since.elapsed() >= ICON_RETRY);
        let icon = match &self.look {
            Some(look) if !retry => look.icon,
            _ => icon_path.as_ref().is_some_and(|path| path.exists()),
        };
        self.no_icon_since = if icon {
            None
        } else if retry {
            Some(Instant::now())
        } else {
            self.no_icon_since
        };

        let look = Look {
            title: title.to_owned(),
            app_id: app_id.to_owned(),
            width: (width * scale).round() as i32,
            scale: scale.to_bits(),
            active,
            color: color.to_array_unpremul().map(|c| (c * 255.).round() as u8),
            font: FONT.with(|font| font.borrow().clone()),
            corners: crate::pixel_corners::pixel().to_bits(),
            icon,
            mac: mac(),
        };
        if self.look.as_ref() != Some(&look) {
            self.buffer = draw(renderer, &look, scale, color, icon_path.filter(|_| icon))
                .map_err(|err| warn!("error drawing a title bar: {err:?}"))
                .ok();
            self.look = Some(look);
        }
        self.buffer.clone()
    }
}

fn icon_path(app_id: &str) -> Option<PathBuf> {
    if app_id.is_empty() || app_id.contains('/') {
        return None;
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(dir).join(format!("angel-niri/title-icons/{app_id}.png")))
}

fn mix(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

const WHITE: [f64; 3] = [1., 1., 1.];
const BLACK: [f64; 3] = [0., 0., 0.];

/// The button glyphs, eight by seven art pixels.
const GLYPHS: [[&str; 7]; 3] = [
    [
        "........", "........", "........", "........", "........", "..####..", "........",
    ],
    [
        "........", ".######.", ".######.", ".#....#.", ".#....#.", ".######.", "........",
    ],
    [
        "........", ".##..##.", "..####..", "...##...", "..####..", ".##..##.", "........",
    ],
];

fn draw(
    renderer: &mut GlesRenderer,
    look: &Look,
    scale: f64,
    color: Color,
    icon: Option<PathBuf>,
) -> anyhow::Result<TextureBuffer<GlesTexture>> {
    let width = look.width.max(1);
    let height = ((TITLE_HEIGHT * scale).round() as i32).max(1);
    // one art pixel in physical pixels, never blurred over two
    let ap = (ART * scale).round().max(1.);
    let logical_width = f64::from(width) / scale;

    let [r, g, b, _] = color.to_array_unpremul().map(f64::from);
    let accent = [r, g, b];
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let bg = if look.active {
        accent
    } else {
        mix([lum * 0.5; 3], accent, 0.2)
    };
    let bg_lum = 0.2126 * bg[0] + 0.7152 * bg[1] + 0.0722 * bg[2];
    let text = if bg_lum > 0.5 { BLACK } else { WHITE };
    let face = mix(bg, WHITE, 0.65);
    let light = mix(face, WHITE, 0.7);
    let shadow = mix(bg, BLACK, 0.55);
    let glyph = mix(bg, BLACK, 0.85);

    if look.mac {
        return draw_mac(renderer, look, scale);
    }

    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    cr.set_antialias(cairo::Antialias::None);
    let rect = |cr: &cairo::Context, c: [f64; 3], x: f64, y: f64, w: f64, h: f64| {
        cr.set_source_rgb(c[0], c[1], c[2]);
        cr.rectangle(x, y, w, h);
        let _ = cr.fill();
    };

    rect(&cr, bg, 0., 0., f64::from(width), f64::from(height));

    // the top corners cut in steps like a pixel window's (Settings → Windows → Corners)
    let corner = f64::from(f32::from_bits(look.corners)) * scale;
    if corner > 0. {
        let steps = 2.;
        let step = corner.round().max(1.);
        cr.set_operator(cairo::Operator::Clear);
        for i in 0..steps as i32 {
            let i = f64::from(i);
            let w = step * (steps - i);
            cr.rectangle(0., step * i, w, step);
            cr.rectangle(f64::from(width) - w, step * i, w, step);
        }
        let _ = cr.fill();
        cr.set_operator(cairo::Operator::Over);
    }

    // the buttons, raised one art pixel
    for (glyph_rows, (_, button)) in GLYPHS.iter().zip(buttons(logical_width)) {
        let x = (button.loc.x / ART).round() * ap;
        let y = (button.loc.y / ART).round() * ap;
        let (w, h) = (8. * ap, 7. * ap);
        rect(&cr, shadow, x, y, w, h);
        rect(&cr, light, x, y, w - ap, h - ap);
        rect(&cr, face, x + ap, y + ap, w - 2. * ap, h - 2. * ap);
        for (row, line) in glyph_rows.iter().enumerate() {
            for (col, c) in line.chars().enumerate() {
                if c == '#' {
                    rect(&cr, glyph, x + col as f64 * ap, y + row as f64 * ap, ap, ap);
                }
            }
        }
    }

    // the icon
    let mut text_x = 3. * ap;
    if let Some(path) = icon {
        match load_icon(&path) {
            Ok(icon) => {
                let size = 8. * ap;
                let (iw, ih) = (f64::from(icon.width()), f64::from(icon.height()));
                cr.save()?;
                cr.translate(2. * ap, 2. * ap);
                cr.scale(size / iw.max(1.), size / ih.max(1.));
                cr.set_source_surface(&icon, 0., 0.)?;
                cr.source().set_filter(cairo::Filter::Nearest);
                cr.paint()?;
                cr.restore()?;
                text_x = 12. * ap;
            }
            Err(err) => debug!("no title icon from {path:?}: {err:?}"),
        }
    }

    // the title, ellipsized before the buttons
    let font = if look.font.is_empty() {
        "monospace 12"
    } else {
        &look.font
    };
    let mut font = FontDescription::from_string(font);
    font.set_absolute_size((font.size() as f64 * scale).round());
    let mut options = cairo::FontOptions::new()?;
    options.set_antialias(cairo::Antialias::None);
    options.set_hint_style(cairo::HintStyle::None);
    let layout = pangocairo::functions::create_layout(&cr);
    pangocairo::functions::context_set_font_options(&layout.context(), Some(&options));
    layout.set_font_description(Some(&font));
    layout.set_ellipsize(EllipsizeMode::End);
    let text_end = (buttons(logical_width)[0].1.loc.x / ART).round() * ap - 2. * ap;
    let text_width = (text_end - text_x).max(0.);
    layout.set_width((text_width * f64::from(pangocairo::pango::SCALE)) as i32);
    layout.set_text(&look.title);
    let (_, text_height) = layout.pixel_size();
    let text_y = ((f64::from(height) - f64::from(text_height)) / 2.).round();
    cr.set_source_rgb(text[0], text[1], text[2]);
    cr.move_to(text_x, text_y);
    pangocairo::functions::show_layout(&cr, &layout);
    drop(cr);

    let data = surface.take_data()?;
    Ok(TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width, height),
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?)
}

/// A round button, six by six art pixels.
const MAC_BUTTON: [&str; 6] = ["..##..", ".####.", "######", "######", ".####.", "..##.."];

/// The Mac look (see the module docs).
fn draw_mac(
    renderer: &mut GlesRenderer,
    look: &Look,
    scale: f64,
) -> anyhow::Result<TextureBuffer<GlesTexture>> {
    let width = look.width.max(1);
    let height = ((TITLE_HEIGHT * scale).round() as i32).max(1);
    let ap = (ART * scale).round().max(1.);

    let bg = if look.active { [0.91; 3] } else { [0.96; 3] };
    let text = if look.active { [0.15; 3] } else { [0.55; 3] };
    let lights: [[f64; 3]; 3] = if look.active {
        [[1., 0.37, 0.34], [1., 0.74, 0.18], [0.16, 0.78, 0.25]]
    } else {
        [[0.8; 3]; 3]
    };

    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    cr.set_antialias(cairo::Antialias::None);
    let rect = |cr: &cairo::Context, c: [f64; 3], x: f64, y: f64, w: f64, h: f64| {
        cr.set_source_rgb(c[0], c[1], c[2]);
        cr.rectangle(x, y, w, h);
        let _ = cr.fill();
    };
    rect(&cr, bg, 0., 0., f64::from(width), f64::from(height));
    // a one art pixel line under the bar
    rect(
        &cr,
        [0.78; 3],
        0.,
        f64::from(height) - ap,
        f64::from(width),
        ap,
    );

    let corner = f64::from(f32::from_bits(look.corners)) * scale;
    if corner > 0. {
        let step = corner.round().max(1.);
        cr.set_operator(cairo::Operator::Clear);
        for i in 0..2 {
            let i = f64::from(i);
            let w = step * (2. - i);
            cr.rectangle(0., step * i, w, step);
            cr.rectangle(f64::from(width) - w, step * i, w, step);
        }
        let _ = cr.fill();
        cr.set_operator(cairo::Operator::Over);
    }

    for (color, (_, button)) in lights.iter().zip(mac_buttons()) {
        let x = (button.loc.x / ART).round() * ap;
        let y = (button.loc.y / ART).round() * ap;
        for (row, line) in MAC_BUTTON.iter().enumerate() {
            for (col, c) in line.chars().enumerate() {
                if c == '#' {
                    rect(
                        &cr,
                        *color,
                        x + col as f64 * ap,
                        y + row as f64 * ap,
                        ap,
                        ap,
                    );
                }
            }
        }
    }

    // the title in the middle, clear of the buttons on both sides
    let font = if look.font.is_empty() {
        "sans 12"
    } else {
        &look.font
    };
    let mut font = FontDescription::from_string(font);
    font.set_absolute_size((font.size() as f64 * scale).round());
    let mut options = cairo::FontOptions::new()?;
    options.set_antialias(cairo::Antialias::None);
    options.set_hint_style(cairo::HintStyle::None);
    let layout = pangocairo::functions::create_layout(&cr);
    pangocairo::functions::context_set_font_options(&layout.context(), Some(&options));
    layout.set_font_description(Some(&font));
    layout.set_ellipsize(EllipsizeMode::End);
    layout.set_alignment(pangocairo::pango::Alignment::Center);
    let margin = (28. * ap).min(f64::from(width) / 2.);
    let text_width = (f64::from(width) - 2. * margin).max(0.);
    layout.set_width((text_width * f64::from(pangocairo::pango::SCALE)) as i32);
    layout.set_text(&look.title);
    let (_, text_height) = layout.pixel_size();
    let text_y = ((f64::from(height) - f64::from(text_height)) / 2.).round();
    cr.set_source_rgb(text[0], text[1], text[2]);
    cr.move_to(margin, text_y);
    pangocairo::functions::show_layout(&cr, &layout);
    drop(cr);

    let data = surface.take_data()?;
    Ok(TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width, height),
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?)
}

/// A PNG icon as a cairo surface (premultiplied ARGB).
fn load_icon(path: &PathBuf) -> anyhow::Result<ImageSurface> {
    let file = std::fs::File::open(path)?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buf)?;
    let (w, h) = (info.width as i32, info.height as i32);
    anyhow::ensure!(w > 0 && h > 0 && w <= 512 && h <= 512, "odd size {w}×{h}");

    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        other => anyhow::bail!("colour type {other:?}"),
    };
    let mut surface = ImageSurface::create(cairo::Format::ARgb32, w, h)?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface.data()?;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let p = &buf[(y * w as usize + x) * channels..][..channels];
                let (r, g, b, a) = match channels {
                    4 => (p[0], p[1], p[2], p[3]),
                    3 => (p[0], p[1], p[2], 255),
                    2 => (p[0], p[0], p[0], p[1]),
                    _ => (p[0], p[0], p[0], 255),
                };
                let pre = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                let o = y * stride + x * 4;
                // cairo's ARGB32 is native-endian: B G R A in memory on little-endian
                data[o..o + 4].copy_from_slice(&[pre(b), pre(g), pre(r), a]);
            }
        }
    }
    Ok(surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_parts() {
        let size = Size::from((400., 300.));
        let at = |x: f64, y: f64| frame_part(Point::from((x, y)), size);
        assert_eq!(at(100., 10.), Some(FramePart::Title));
        assert_eq!(at(100., 100.), None, "the window itself");
        // close: 2 art px from the right, 8 wide; maximize and minimize to its left
        assert_eq!(at(400. - 4. - 8., 10.), Some(FramePart::Close));
        assert_eq!(
            at(400. - 4. - 16. - 4. - 8., 10.),
            Some(FramePart::Maximize)
        );
        assert_eq!(
            at(400. - 4. - 16. - 4. - 16. - 8., 10.),
            Some(FramePart::Minimize)
        );
        assert_eq!(at(-3., 150.), Some(FramePart::Edge(ResizeEdge::LEFT)));
        assert_eq!(at(200., 303.), Some(FramePart::Edge(ResizeEdge::BOTTOM)));
        assert_eq!(at(-3., 5.), Some(FramePart::Edge(ResizeEdge::TOP_LEFT)));
        assert_eq!(at(-10., 150.), None, "beyond the grab");
    }

    #[test]
    fn mac_frame_parts() {
        set_mac(true);
        let size = Size::from((400., 300.));
        let at = |x: f64, y: f64| frame_part(Point::from((x, y)), size);
        // close, minimize, maximize from the left: 4 art px in, 6 wide, 2 apart
        assert_eq!(at(10., 10.), Some(FramePart::Close));
        assert_eq!(at(26., 10.), Some(FramePart::Minimize));
        assert_eq!(at(42., 10.), Some(FramePart::Maximize));
        assert_eq!(at(390., 10.), Some(FramePart::Title));
        set_mac(false);
    }
}
