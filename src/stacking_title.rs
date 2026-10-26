//! angel-niri (stacking.rs): the title bars of windows while stacking.
//!
//! A floating window that left its decorations to the compositor (xdg-decoration server side) gets
//! a pixel title bar: three buttons as Windows 11 draws them — the glyphs × _ □ in the title's colour
//! over a shadow, a plate under the one the pointer is over (red under ×), close, minimize, maximize
//! from the left (`--title-buttons right`: minimize, maximize, close up to the right) — and its icon
//! and title in the middle in the shell's title font, all on the art-pixel grid. The active window's
//! bar is in the focus ring's active colour, the others' in a dim grey of it; a shell that
//! recolours the ring recolours them too.
//! Windows that draw their own title bars get none.
//!
//! The icon is the shell's: `$XDG_RUNTIME_DIR/angel-niri/title-icons/<app id>.png`, drawn eight
//! art pixels square with nearest filtering before the title.
//!
//! Around such a window the pointer also grabs its edges to resize it, `GRAB` px outside.
//!
//! A window kept on top or on every workspace (desk_flags.rs) shows a small pixel badge for it on
//! the right — a pin, two desks — and a click on the badge clears that flag.
//!
//! The Mac look (`--title-style mac`, Golden Gate) has the same buttons in Mac colours on a light
//! grey bar, the title without an icon.

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
    static LIGHT_THEME: Cell<bool> = const { Cell::new(false) };
    static RIGHT: Cell<bool> = const { Cell::new(false) };
}

/// Whether the buttons sit on the right (`--title-buttons right`), the badges then on the left.
pub fn set_right(right: bool) {
    RIGHT.with(|cell| cell.set(right));
}

fn right() -> bool {
    RIGHT.with(Cell::get)
}

/// Whether the pixel bars are for a light theme (`--title-style light`): the inactive ones pale.
pub fn set_light(light: bool) {
    LIGHT_THEME.with(|cell| cell.set(light));
}

fn light() -> bool {
    LIGHT_THEME.with(Cell::get)
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
    /// The badges of a window on top and on every workspace.
    OnTop,
    Sticky,
    Edge(ResizeEdge),
}

/// What a tile's frame has besides the title bar and the buttons.
#[derive(Debug, Clone, Copy, Default)]
pub struct Extras {
    /// Its edges resize it (not while shaded: then the bar gets a bottom edge of its own).
    pub edges: bool,
    pub on_top: bool,
    pub sticky: bool,
}

/// The badges of a bar of `width`, from the side away from the buttons: six art pixels square, two
/// apart, four from the edge; only the flags that are on.
fn badges(width: f64, extras: Extras, right: bool) -> Vec<(FramePart, Rectangle<f64, Logical>)> {
    let edge = (width / ART).floor() * ART;
    let flags = [
        (FramePart::OnTop, extras.on_top),
        (FramePart::Sticky, extras.sticky),
    ];
    flags
        .into_iter()
        .filter(|(_, on)| *on)
        .enumerate()
        .map(|(i, (part, _))| {
            let x = if right {
                (4. + 8. * i as f64) * ART
            } else {
                edge - (10. + 8. * i as f64) * ART
            };
            let rect = Rectangle::new(Point::from((x, 3. * ART)), Size::from((6. * ART, 6. * ART)));
            (part, rect)
        })
        .collect()
}

/// From one button to the next, in art pixels: three apart, room for their outlines.
const PITCH: f64 = 9.;

/// A pixel skin's button: the whole bar's height, this many art pixels wide, side by side from the
/// edge (Windows 11's: the corner closes).
const CELL: f64 = 12.;

/// The buttons of a bar of `width`: close, minimize, maximize from the left, or minimize, maximize,
/// close up to the right. The Mac look's are lights six art pixels round, three apart, four from
/// the edge; the pixel skins' are cells (`CELL`).
fn buttons(width: f64, right: bool, mac: bool) -> [(FramePart, Rectangle<f64, Logical>); 3] {
    let edge = (width / ART).floor() * ART;
    let rect = |i: f64| {
        if !mac {
            let x = if right {
                edge - CELL * (3. - i) * ART
            } else {
                CELL * i * ART
            };
            return Rectangle::new(Point::from((x, 0.)), Size::from((CELL * ART, TITLE_HEIGHT)));
        }
        let x = if right {
            edge - (10. + PITCH * (2. - i)) * ART
        } else {
            (4. + PITCH * i) * ART
        };
        Rectangle::new(Point::from((x, 3. * ART)), Size::from((6. * ART, 6. * ART)))
    };
    if right {
        [
            (FramePart::Minimize, rect(0.)),
            (FramePart::Maximize, rect(1.)),
            (FramePart::Close, rect(2.)),
        ]
    } else {
        [
            (FramePart::Close, rect(0.)),
            (FramePart::Minimize, rect(1.)),
            (FramePart::Maximize, rect(2.)),
        ]
    }
}

/// The span of x the buttons of a bar of `width` take, from the left of the bar.
pub fn buttons_span(width: f64) -> (f64, f64) {
    let rects = buttons(width, right(), mac());
    let x0 = rects.iter().map(|(_, r)| r.loc.x).fold(f64::MAX, f64::min);
    let x1 = rects
        .iter()
        .map(|(_, r)| r.loc.x + r.size.w)
        .fold(f64::MIN, f64::max);
    (x0, x1)
}

/// Which part of the frame of a titled tile of `size` is at `pos` (relative to the tile, the
/// title bar on top): its title bar and buttons, or an edge up to `GRAB` px outside. `None` over
/// the window itself and beyond.
pub fn frame_part(
    pos: Point<f64, Logical>,
    size: Size<f64, Logical>,
    extras: Extras,
) -> Option<FramePart> {
    let (x, y, w, h) = (pos.x, pos.y, size.w, size.h);
    if x < -GRAB || y < -GRAB || x >= w + GRAB || y >= h + GRAB {
        return None;
    }

    let out_x = x < 0. || x >= w;
    let out_y = y < 0. || y >= h;
    if out_x || out_y {
        if !extras.edges {
            return None;
        }
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
    let part = buttons(w, right(), mac())
        .into_iter()
        .chain(badges(w, extras, right()))
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
    light: bool,
    right: bool,
    hover: Option<FramePart>,
    on_top: bool,
    sticky: bool,
    shaded: bool,
    /// The shell's notch over the bar, a span of x in physical px (stacking::notch_over_bar).
    notch: Option<(i32, i32)>,
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
    /// ring's active colour, `hover` the button the pointer is over, `notch` the span of the bar
    /// under the shell's notch.
    #[allow(clippy::too_many_arguments)]
    pub fn texture(
        &mut self,
        renderer: &mut GlesRenderer,
        title: &str,
        app_id: &str,
        width: f64,
        scale: f64,
        active: bool,
        hover: Option<FramePart>,
        extras: Extras,
        color: Color,
        notch: Option<(f64, f64)>,
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
            light: light(),
            right: right(),
            hover,
            on_top: extras.on_top,
            sticky: extras.sticky,
            shaded: !extras.edges,
            notch: notch.map(|(x0, x1)| ((x0 * scale).round() as i32, (x1 * scale).round() as i32)),
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

/// WCAG relative luminance of an sRGB colour.
fn luminance(c: [f64; 3]) -> f64 {
    let lin = |v: f64| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// WCAG contrast ratio of two colours.
fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Black or white, whichever reads better on `bg`.
fn ink(bg: [f64; 3]) -> [f64; 3] {
    if contrast(BLACK, bg) >= contrast(WHITE, bg) {
        BLACK
    } else {
        WHITE
    }
}

const WHITE: [f64; 3] = [1., 1., 1.];
const BLACK: [f64; 3] = [0., 0., 0.];

/// A round button, six by six art pixels.
const LIGHT: [&str; 6] = ["..##..", ".####.", "######", "######", ".####.", "..##.."];

/// The glyphs on the buttons while the pointer is over them, four by four art pixels in the
/// middle of each: close (×), minimize (–), maximize (+), all centred.
const GLYPHS: [[&str; 4]; 3] = [
    ["#..#", ".##.", ".##.", "#..#"],
    ["....", "####", "####", "...."],
    [".##.", "####", "####", ".##."],
];

/// The pixel skins' glyphs, seven by seven art pixels in the text's colour over a shadow: minimize
/// (_), maximize (□), close (×), as Windows 11 draws them.
const CELL_GLYPHS: [[&str; 7]; 3] = [
    [
        "#.....#", ".#...#.", "..#.#..", "...#...", "..#.#..", ".#...#.", "#.....#",
    ],
    [
        ".......", ".......", ".......", ".......", ".......", ".......", "#######",
    ],
    [
        "#######", "#.....#", "#.....#", "#.....#", "#.....#", "#.....#", "#######",
    ],
];

/// Windows' red under the close button the pointer is over.
const CLOSE_RED: [f64; 3] = [0.91, 0.25, 0.22];

/// The badges, six by six art pixels, as the shell's menu draws them (widgets/Icons.js arrowUp,
/// allDesks): an arrow up (on top), two desks, the front one filled (on every workspace).
const BADGES: [[&str; 6]; 2] = [
    ["..##..", ".####.", "######", "..##..", "..##..", "..##.."],
    ["..####", "..#..#", "####.#", "######", "####..", "####.."],
];

/// The Mac look's red, yellow and green.
const MAC_LIGHTS: [[f64; 3]; 3] = [[1., 0.37, 0.34], [1., 0.74, 0.18], [0.16, 0.78, 0.25]];

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

    let (bg, text, line) = if look.mac {
        let bg = if look.active { [0.91; 3] } else { [0.96; 3] };
        let text = if look.active { [0.15; 3] } else { [0.4; 3] };
        (bg, text, Some([0.78; 3]))
    } else {
        let [r, g, b, _] = color.to_array_unpremul().map(f64::from);
        let accent = [r, g, b];
        let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let bg = if look.active {
            accent
        } else if look.light {
            mix([0.88; 3], accent, 0.18)
        } else {
            mix([lum * 0.5; 3], accent, 0.2)
        };
        (bg, ink(bg), None)
    };
    // the Mac look's lights
    let lights = if look.active {
        MAC_LIGHTS
    } else {
        [[0.7; 3]; 3]
    };

    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    cr.set_antialias(cairo::Antialias::None);
    let rect = |cr: &cairo::Context, c: [f64; 3], x: f64, y: f64, w: f64, h: f64| {
        cr.set_source_rgb(c[0], c[1], c[2]);
        cr.rectangle(x, y, w, h);
        let _ = cr.fill();
    };
    let sprite = |cr: &cairo::Context, c: [f64; 3], x: f64, y: f64, rows: &[&str]| {
        for (row, line) in rows.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                if ch == '#' {
                    rect(cr, c, x + col as f64 * ap, y + row as f64 * ap, ap, ap);
                }
            }
        }
    };

    rect(&cr, bg, 0., 0., f64::from(width), f64::from(height));
    if let Some(line) = line {
        // a one art pixel line under the bar
        rect(&cr, line, 0., f64::from(height) - ap, f64::from(width), ap);
    } else if look.shaded {
        // rolled up: the bar's own bottom edge, so it does not read as part of the window behind
        rect(
            &cr,
            mix(bg, BLACK, 0.5),
            0.,
            f64::from(height) - ap,
            f64::from(width),
            ap,
        );
    }

    let logical_width = f64::from(width) / scale;
    for (part, button) in buttons(logical_width, look.right, look.mac) {
        let i = match part {
            FramePart::Close => 0,
            FramePart::Minimize => 1,
            _ => 2,
        };
        let x = (button.loc.x / ART).round() * ap;
        let y = (button.loc.y / ART).round() * ap;
        if !look.mac {
            // a glyph in the text's colour with a shadow away from it; under the pointer a plate,
            // the close button's red
            let hover = look.hover == Some(part);
            let mut ink = text;
            if hover {
                let plate = if part == FramePart::Close {
                    CLOSE_RED
                } else {
                    mix(bg, text, 0.2)
                };
                rect(&cr, plate, x, 0., CELL * ap, f64::from(height));
                if part == FramePart::Close {
                    ink = WHITE;
                }
            }
            let bg = if hover && part == FramePart::Close {
                CLOSE_RED
            } else {
                bg
            };
            let shadow = if luminance(ink) > luminance(bg) {
                mix(bg, BLACK, 0.55)
            } else {
                mix(bg, WHITE, 0.6)
            };
            // seven art pixels and the shadow's one in the middle of the cell
            let glyph = &CELL_GLYPHS[i];
            let (gx, gy) = (x + 2. * ap, y + 2. * ap);
            sprite(&cr, shadow, gx + ap, gy + ap, glyph);
            sprite(&cr, ink, gx, gy, glyph);
            continue;
        }
        let (light, glyph) = (&lights[i], GLYPHS[i]);
        sprite(&cr, *light, x, y, &LIGHT);
        if look.hover.is_some() {
            sprite(&cr, mix(*light, ink(*light), 0.75), x + ap, y + ap, &glyph);
        }
    }

    // the top corners cut in steps like a pixel window's (Settings → Windows → Corners), over the
    // buttons' plates
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

    let extras = Extras {
        edges: true,
        on_top: look.on_top,
        sticky: look.sticky,
    };
    for (part, badge) in badges(logical_width, extras, look.right) {
        let rows = if part == FramePart::OnTop {
            &BADGES[0]
        } else {
            &BADGES[1]
        };
        let x = (badge.loc.x / ART).round() * ap;
        let y = (badge.loc.y / ART).round() * ap;
        sprite(&cr, text, x, y, rows);
    }

    // the icon and the title in the middle, clear of the buttons on both sides
    let font = if look.font.is_empty() {
        if look.mac {
            "sans 12"
        } else {
            "monospace 12"
        }
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

    let icon = icon.filter(|_| !look.mac).and_then(|path| {
        load_icon(&path)
            .map_err(|err| debug!("no title icon from {path:?}: {err:?}"))
            .ok()
    });
    let icon_size = 8. * ap;
    let icon_room = if icon.is_some() {
        icon_size + 2. * ap
    } else {
        0.
    };
    // clear of the buttons: three cells of the pixel skins, the Mac's lights; with the shell's
    // notch over the middle, in the wider part beside it
    let margin = (38. * ap).min(f64::from(width) / 2.);
    let (mut lo, mut hi) = (margin, f64::from(width) - margin);
    if let Some((x0, x1)) = look.notch {
        let gap = 4. * ap;
        let (x0, x1) = (f64::from(x0) - gap, f64::from(x1) + gap);
        if x0 - lo >= hi - x1 {
            hi = hi.min(x0);
        } else {
            lo = lo.max(x1);
        }
    }
    let text_room = (hi - lo - icon_room).max(0.);
    layout.set_width((text_room * f64::from(pangocairo::pango::SCALE)) as i32);
    layout.set_text(&look.title);
    let (text_width, text_height) = layout.pixel_size();
    let left = (lo + (hi - lo - icon_room - f64::from(text_width)) / 2.)
        .max(lo)
        .round();

    if let Some(icon) = icon {
        let (iw, ih) = (f64::from(icon.width()), f64::from(icon.height()));
        cr.save()?;
        cr.translate(left, 2. * ap);
        cr.scale(icon_size / iw.max(1.), icon_size / ih.max(1.));
        cr.set_source_surface(&icon, 0., 0.)?;
        cr.source().set_filter(cairo::Filter::Nearest);
        cr.paint()?;
        cr.restore()?;
    }

    let text_y = ((f64::from(height) - f64::from(text_height)) / 2.).round();
    cr.set_source_rgb(text[0], text[1], text[2]);
    cr.move_to(left + icon_room, text_y);
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
        let extras = Extras {
            edges: true,
            ..Extras::default()
        };
        let at = |x: f64, y: f64| frame_part(Point::from((x, y)), size, extras);
        assert_eq!(at(100., 10.), Some(FramePart::Title));
        assert_eq!(at(100., 100.), None, "the window itself");
        // close, minimize, maximize from the left: cells 12 art px wide, the bar's height
        assert_eq!(at(1., 1.), Some(FramePart::Close));
        assert_eq!(at(30., 22.), Some(FramePart::Minimize));
        assert_eq!(at(60., 10.), Some(FramePart::Maximize));
        assert_eq!(at(75., 10.), Some(FramePart::Title));
        assert_eq!(at(390., 10.), Some(FramePart::Title));
        assert_eq!(at(-3., 150.), Some(FramePart::Edge(ResizeEdge::LEFT)));
        assert_eq!(at(200., 303.), Some(FramePart::Edge(ResizeEdge::BOTTOM)));
        assert_eq!(at(-3., 5.), Some(FramePart::Edge(ResizeEdge::TOP_LEFT)));
        assert_eq!(at(-10., 150.), None, "beyond the grab");

        // on the right, like Windows: minimize, maximize, close, the corner closes
        set_right(true);
        assert_eq!(at(399., 1.), Some(FramePart::Close));
        assert_eq!(at(400. - 30., 10.), Some(FramePart::Maximize));
        assert_eq!(at(400. - 60., 10.), Some(FramePart::Minimize));
        assert_eq!(at(10., 10.), Some(FramePart::Title));
        set_right(false);

        // the Mac's lights: 4 art px in, 6 wide, 3 apart
        set_mac(true);
        assert_eq!(at(10., 10.), Some(FramePart::Close));
        assert_eq!(at(28., 10.), Some(FramePart::Minimize));
        assert_eq!(at(46., 10.), Some(FramePart::Maximize));
        assert_eq!(at(1., 1.), Some(FramePart::Title));
        set_mac(false);
    }

    #[test]
    fn glyphs_stand_apart_from_every_bar() {
        // hell's circles and heaven's flavours (the design review): the title and the buttons'
        // glyphs are in ink()
        for hex in [
            0xe2703f, 0xec7650, 0xec6a5a, 0xcfa64c, 0xa3bd4e, 0xfabd2f, 0x87b0b3, 0x58b8a0,
        ] {
            let bg = [16, 8, 0].map(|s| f64::from((hex >> s) & 0xff) / 255.);
            assert!(contrast(ink(bg), bg) >= 4.5, "{hex:06x}");
        }
    }

    #[test]
    fn badges_and_shaded_edges() {
        let size = Size::from((400., 24.));
        let extras = Extras {
            edges: false,
            on_top: true,
            sticky: true,
        };
        let at = |x: f64, y: f64| frame_part(Point::from((x, y)), size, extras);
        // the pin 4 art px from the right, 6 wide; the desks 2 art px to its left
        assert_eq!(at(400. - 10., 10.), Some(FramePart::OnTop));
        assert_eq!(at(400. - 26., 10.), Some(FramePart::Sticky));
        assert_eq!(at(-3., 10.), None, "a shaded window does not resize");
    }
}
