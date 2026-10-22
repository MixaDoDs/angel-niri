//! angel-niri: pixel overview. `niri msg action set-pixel-overview PIXEL [--steps N] [--font F]`
//! (0 = off), for a shell with pixel-art decorations; nothing goes to niri's config:
//! - the thumbnails are scaled down with nearest filtering: sharp pixels, no blur;
//! - zooming in and out goes in `steps` steps instead of smoothly, and so does a workspace switch;
//! - every workspace gets a frame of 2 × PIXEL logical px with notched corners, the active one in
//!   the focus ring's active colour, and its name (or number) left of it in the font `F` ("Family
//!   SIZE", size in logical px; a pixel font is drawn without antialiasing).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use niri_config::Color;
use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::FontDescription;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Renderer as _, TextureFilter};
use smithay::utils::{Point, Rectangle, Size, Transform};

use crate::layout::monitor::Monitor;
use crate::layout::LayoutElement;
use crate::niri::OutputRenderElements;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::utils::to_physical_precise_round;

static PIXEL: AtomicU32 = AtomicU32::new(0);
static STEPS: AtomicU32 = AtomicU32::new(6);
static FONT: Mutex<String> = Mutex::new(String::new());

/// The art pixel, logical px; 0 when off.
pub fn pixel() -> f32 {
    f32::from_bits(PIXEL.load(Ordering::Relaxed))
}

pub fn set(pixel: f32, steps: u32, font: String) {
    PIXEL.store(pixel.max(0.).to_bits(), Ordering::Relaxed);
    STEPS.store(steps.max(1), Ordering::Relaxed);
    *FONT.lock().unwrap() = font;
    CACHE.with_borrow_mut(|cache| cache.labels.clear());
}

/// The overview's zoom progress, in steps while pixel overview is on.
pub fn step(progress: f64) -> f64 {
    if pixel() <= 0. {
        return progress;
    }
    let steps = f64::from(STEPS.load(Ordering::Relaxed));
    (progress * steps).round() / steps
}

/// A value animated from `from` to `to`, in steps while pixel overview is on (a workspace switch).
pub fn step_between(from: f64, to: f64, value: f64) -> f64 {
    if (to - from).abs() < f64::EPSILON {
        return value;
    }
    from + step((value - from) / (to - from)) * (to - from)
}

/// Nearest filtering for the thumbnails while `output`'s overview shows; linear otherwise.
/// The filter holds for everything drawn until the next call, i.e. this output's frame.
pub fn set_filter(renderer: &mut GlesRenderer, overview: bool) {
    let filter = if overview && pixel() > 0. {
        TextureFilter::Nearest
    } else {
        TextureFilter::Linear
    };
    if let Err(err) = renderer.downscale_filter(filter) {
        warn!("error setting the downscale filter: {err:?}");
    }
}

#[derive(Default)]
struct Cache {
    // (text, scale, colour) → its texture
    labels: HashMap<(String, u64, [u8; 4]), Option<TextureBuffer<GlesTexture>>>,
    // (output, workspace id) → the frame's four bars
    frames: HashMap<(String, u64), [SolidColorBuffer; 4]>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
}

/// The frames and names of `mon`'s workspaces while its overview shows.
pub fn render<R: NiriRenderer, W: LayoutElement>(
    renderer: &mut R,
    mon: &Monitor<W>,
    output_name: &str,
    active: Color,
    inactive: Color,
    push: &mut dyn FnMut(OutputRenderElements<R>),
) {
    let px = f64::from(pixel());
    let Some(progress) = mon.overview_clamped_progress().filter(|_| px > 0.) else {
        return;
    };
    let alpha = progress.clamp(0., 1.) as f32;
    let scale = mon.scale().fractional_scale();
    let round = |v: f64| (v * scale).round() / scale;
    let active_id = mon.active_workspace_ref().id();

    CACHE.with_borrow_mut(|cache| {
        // shortcut: frames of removed workspaces are only dropped all at once
        if cache.frames.len() > 64 {
            cache.frames.clear();
        }

        for (idx, (ws, geo)) in mon.workspaces_with_render_geo().enumerate() {
            let color = if ws.id() == active_id {
                active
            } else {
                inactive
            };
            let color32 = color.to_array_premul();

            // the frame: one art pixel off the workspace, two thick, corners notched
            let t = round(px * 2.);
            let x = round(geo.loc.x - px - t);
            let y = round(geo.loc.y - px - t);
            let w = round(geo.size.w + (px + t) * 2.);
            let h = round(geo.size.h + (px + t) * 2.);
            let bars = [
                Rectangle::new(Point::from((x + t, y)), Size::from((w - t * 2., t))),
                Rectangle::new(Point::from((x + t, y + h - t)), Size::from((w - t * 2., t))),
                Rectangle::new(Point::from((x, y + t)), Size::from((t, h - t * 2.))),
                Rectangle::new(Point::from((x + w - t, y + t)), Size::from((t, h - t * 2.))),
            ];
            let buffers = cache
                .frames
                .entry((output_name.to_owned(), ws.id().get()))
                .or_default();
            for (buffer, bar) in buffers.iter_mut().zip(bars) {
                buffer.update(bar.size, color32);
                let elem =
                    SolidColorRenderElement::from_buffer(buffer, bar.loc, alpha, Kind::Unspecified);
                push(elem.into());
            }

            // its name
            let text = ws.name().cloned().unwrap_or_else(|| (idx + 1).to_string());
            let rgba = color.to_array_unpremul().map(|c| (c * 255.).round() as u8);
            let label = cache
                .labels
                .entry((text.clone(), scale.to_bits(), rgba))
                .or_insert_with(|| {
                    render_label(renderer.as_gles_renderer(), &text, scale, color)
                        .map_err(|err| warn!("error rendering a workspace label: {err:?}"))
                        .ok()
                });
            if let Some(buffer) = label.clone() {
                let size = buffer.logical_size();
                // left of the frame, at its top: the gap between workspaces can be lower than it
                let loc = Point::from((round(x - px * 2. - size.w), y));
                let elem = TextureRenderElement::from_texture_buffer(
                    buffer,
                    loc,
                    alpha,
                    None,
                    None,
                    Kind::Unspecified,
                );
                push(PrimaryGpuTextureRenderElement(elem).into());
            }
        }
    });
}

fn render_label(
    renderer: &mut GlesRenderer,
    text: &str,
    scale: f64,
    color: Color,
) -> anyhow::Result<TextureBuffer<GlesTexture>> {
    let font = FONT.lock().unwrap().clone();
    let mut font = FontDescription::from_string(if font.is_empty() { "sans 16" } else { &font });
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    // pixel fonts stay sharp: no antialiasing, no hinting that would move their pixels
    let mut options = cairo::FontOptions::new()?;
    options.set_antialias(cairo::Antialias::None);
    options.set_hint_style(cairo::HintStyle::None);

    let layout_for = |cr: &cairo::Context| {
        let layout = pangocairo::functions::create_layout(cr);
        pangocairo::functions::context_set_font_options(&layout.context(), Some(&options));
        layout.set_font_description(Some(&font));
        layout.set_text(text);
        layout
    };

    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let (width, height) = layout_for(&cr).pixel_size();

    let surface = ImageSurface::create(cairo::Format::ARgb32, width.max(1), height.max(1))?;
    let cr = cairo::Context::new(&surface)?;
    let [r, g, b, _] = color.to_array_unpremul();
    cr.set_source_rgb(r.into(), g.into(), b.into());
    pangocairo::functions::show_layout(&cr, &layout_for(&cr));
    drop(cr);

    let data = surface.take_data().unwrap();
    Ok(TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width.max(1), height.max(1)),
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?)
}
