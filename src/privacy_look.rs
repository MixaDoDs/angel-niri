//! angel-niri: what a blocked-out window shows in a screencast (and in screenshots, with
//! block-out-from "screen-capture") instead of stock niri's black. `niri msg action
//! set-privacy-look LOOK [--app-id ID] …`:
//! - `black`: as stock niri;
//! - `mosaic`: the window itself in blocks of `--cell` logical px, drawn sharp, nothing in it
//!   readable (its popups stay hidden);
//! - `image`: a PNG (`--image`) on `--background`: centred, scaled up in whole steps and drawn
//!   sharp (pixel art: a plate, a sprite), or with `--cover` filling the window, smooth (a photo);
//! - `default` (with `--app-id`): the app goes back to the common look.
//! One look for every blocked-out window and one of its own for any app-id. A shell sets them;
//! nothing goes to niri's config.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context as _};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{ContextId, Renderer as _, Texture as _};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};
use crate::render_helpers::render_to_texture;
use crate::render_helpers::renderer::{AsGlesFrame as _, NiriRenderer};
use crate::render_helpers::shaders::Shaders;
use crate::render_helpers::surface::push_elements_from_surface_tree;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};

#[derive(Debug, Clone)]
pub enum Look {
    Black,
    Mosaic {
        cell: f64,
    },
    Image {
        picture: Rc<Picture>,
        cover: bool,
        background: [f32; 4],
    },
}

#[derive(Debug)]
pub struct Picture {
    id: u64,
    size: Size<i32, Buffer>,
    /// Premultiplied RGBA.
    rgba: Vec<u8>,
}

#[derive(Default)]
struct Looks {
    common: Option<Look>,
    apps: HashMap<String, Look>,
}

thread_local! {
    static LOOKS: RefCell<Looks> = RefCell::new(Looks::default());
    /// The pictures' textures: one import for every window that shows it.
    static TEXTURES: RefCell<HashMap<u64, (ContextId<GlesTexture>, GlesTexture)>> =
        RefCell::new(HashMap::new());
}

static NEXT_PICTURE: AtomicU64 = AtomicU64::new(1);

/// The look a window of this app gets.
pub fn look_for(app_id: Option<&str>) -> Look {
    LOOKS.with_borrow(|looks| {
        app_id
            .and_then(|id| looks.apps.get(id))
            .or(looks.common.as_ref())
            .cloned()
            .unwrap_or(Look::Black)
    })
}

pub fn set(
    look: &str,
    app_id: Option<String>,
    image: Option<&str>,
    cell: f64,
    cover: bool,
    background: Option<&str>,
) -> anyhow::Result<()> {
    let look = match look {
        "black" => Some(Look::Black),
        "mosaic" => Some(Look::Mosaic {
            cell: cell.clamp(2., 256.),
        }),
        "image" => {
            let path = image.context("image needs --image PATH")?;
            let picture = load_png(path).with_context(|| format!("error loading {path}"))?;
            let background = match background {
                Some(hex) => parse_color(hex).context("--background is #rrggbb or #rrggbbaa")?,
                None => [0., 0., 0., 1.],
            };
            Some(Look::Image {
                picture: Rc::new(picture),
                cover,
                background,
            })
        }
        "default" if app_id.is_some() => None,
        _ => bail!("look is black, mosaic, image or (with --app-id) default"),
    };

    LOOKS.with_borrow_mut(|looks| match (app_id, look) {
        (Some(id), Some(look)) => {
            looks.apps.insert(id, look);
        }
        (Some(id), None) => {
            looks.apps.remove(&id);
        }
        (None, look) => looks.common = look,
    });

    // Drop the textures of pictures nobody shows any more.
    let used: Vec<u64> = LOOKS.with_borrow(|looks| {
        looks
            .common
            .iter()
            .chain(looks.apps.values())
            .filter_map(|look| match look {
                Look::Image { picture, .. } => Some(picture.id),
                _ => None,
            })
            .collect()
    });
    TEXTURES.with_borrow_mut(|textures| textures.retain(|id, _| used.contains(id)));

    Ok(())
}

fn load_png(path: &str) -> anyhow::Result<Picture> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::ALPHA | png::Transformations::STRIP_16,
    );
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("image too big")?];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());

    let mut rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|px| [px[0], px[0], px[0], px[1]])
            .collect(),
        other => bail!("unexpected colour type {other:?}"),
    };
    for px in rgba.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }

    Ok(Picture {
        id: NEXT_PICTURE.fetch_add(1, Ordering::Relaxed),
        size: Size::from((info.width as i32, info.height as i32)),
        rgba,
    })
}

fn parse_color(hex: &str) -> Option<[f32; 4]> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if !matches!(hex.len(), 6 | 8) || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    let a = f32::from(a) / 255.;
    // Premultiplied, like everything niri draws.
    Some([
        f32::from(byte(0)?) / 255. * a,
        f32::from(byte(2)?) / 255. * a,
        f32::from(byte(4)?) / 255. * a,
        a,
    ])
}

/// What one window keeps between frames, so an unchanged look is not damage.
#[derive(Debug, Default)]
pub struct WindowLook {
    mosaic: Option<(
        Vec<(Id, CommitCounter)>,
        Size<f64, Logical>,
        f64,
        TextureBuffer<GlesTexture>,
    )>,
    picture: Option<(u64, TextureBuffer<GlesTexture>)>,
}

/// Pushes a blocked-out window's look; the background (the window's place in the look's
/// colour) is left to the caller: returns that colour, or `None` when nothing could be drawn
/// and the window stays black.
#[allow(clippy::too_many_arguments)]
pub fn render<R: NiriRenderer>(
    renderer: &mut R,
    look: &Look,
    cache: &mut WindowLook,
    surface: &WlSurface,
    buf_loc: Point<f64, Logical>,
    size: Size<f64, Logical>,
    location: Point<f64, Logical>,
    alpha: f32,
    push: &mut dyn FnMut(PrivacyLookElement),
) -> Option<[f32; 4]> {
    if size.w < 1. || size.h < 1. {
        return None;
    }
    let renderer = renderer.as_gles_renderer();

    match look {
        Look::Black => None,
        Look::Mosaic { cell } => {
            let scale = Scale::from(1. / cell);
            let mut elements = Vec::new();
            push_elements_from_surface_tree(
                renderer,
                surface,
                buf_loc.to_physical_precise_round(scale),
                scale,
                1.,
                Kind::Unspecified,
                &mut |elem: WaylandSurfaceRenderElement<GlesRenderer>| elements.push(elem),
            );
            let key: Vec<_> = elements
                .iter()
                .map(|e| (e.id().clone(), e.current_commit()))
                .collect();

            let fresh = match &cache.mosaic {
                Some((k, s, c, _)) => *k != key || *s != size || c != cell,
                None => true,
            };
            if fresh {
                let tex_size =
                    Size::from(((size.w / cell).ceil() as i32, (size.h / cell).ceil() as i32));
                match render_to_texture(
                    renderer,
                    tex_size,
                    scale,
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements.iter(),
                ) {
                    Ok((texture, _sync_point)) => {
                        let buffer = TextureBuffer::from_texture(
                            renderer,
                            texture,
                            scale,
                            Transform::Normal,
                            vec![],
                        );
                        cache.mosaic = Some((key, size, *cell, buffer));
                    }
                    Err(err) => {
                        warn!("error rendering the privacy mosaic: {err:?}");
                        cache.mosaic = None;
                        return None;
                    }
                }
            }

            let (_, _, _, buffer) = cache.mosaic.as_ref()?;
            let elem = TextureRenderElement::from_texture_buffer(
                buffer.clone(),
                location,
                alpha,
                Some(Rectangle::from_size(size)),
                Some(size),
                Kind::Unspecified,
            );
            push(PrivacyLookElement {
                inner: elem,
                sharp: true,
            });
            Some([0., 0., 0., 1.])
        }
        Look::Image {
            picture,
            cover,
            background,
        } => {
            let texture = TEXTURES.with_borrow_mut(|textures| {
                let context = renderer.context_id();
                if let Some((ctx, texture)) = textures.get(&picture.id) {
                    if *ctx == context {
                        return Some(texture.clone());
                    }
                }
                let buffer = TextureBuffer::from_memory(
                    renderer,
                    &picture.rgba,
                    Fourcc::Abgr8888,
                    picture.size,
                    false,
                    1.,
                    Transform::Normal,
                    vec![],
                )
                .map_err(|err| warn!("error importing the privacy picture: {err:?}"))
                .ok()?;
                let texture = buffer.texture().clone();
                textures.insert(picture.id, (context, texture.clone()));
                Some(texture)
            })?;

            if cache
                .picture
                .as_ref()
                .is_none_or(|(id, _)| *id != picture.id)
            {
                let buffer =
                    TextureBuffer::from_texture(renderer, texture, 1., Transform::Normal, vec![]);
                cache.picture = Some((picture.id, buffer));
            }
            let (_, buffer) = cache.picture.as_ref()?;

            let pic = picture.size.to_f64();
            let elem = if *cover {
                // Fill the window, cut what sticks out evenly on both sides.
                let s = f64::max(size.w / pic.w, size.h / pic.h);
                let src_size = Size::from((size.w / s, size.h / s));
                let src_loc = Point::from(((pic.w - src_size.w) / 2., (pic.h - src_size.h) / 2.));
                let elem = TextureRenderElement::from_texture_buffer(
                    buffer.clone(),
                    location,
                    alpha,
                    Some(Rectangle::new(src_loc, src_size)),
                    Some(size),
                    Kind::Unspecified,
                );
                PrivacyLookElement {
                    inner: elem,
                    sharp: false,
                }
            } else {
                // Whole steps up to about 3/5 of the window; smaller if even one is too big.
                let fit = f64::min(size.w * 0.6 / pic.w, size.h * 0.6 / pic.h);
                let s = if fit >= 1. {
                    fit.floor()
                } else {
                    f64::min(size.w / pic.w, size.h / pic.h).min(1.)
                };
                let drawn = Size::from((pic.w * s, pic.h * s));
                let at = location
                    + Point::from((
                        ((size.w - drawn.w) / 2.).round(),
                        ((size.h - drawn.h) / 2.).round(),
                    ));
                let elem = TextureRenderElement::from_texture_buffer(
                    buffer.clone(),
                    at,
                    alpha,
                    None,
                    Some(drawn),
                    Kind::Unspecified,
                );
                PrivacyLookElement {
                    inner: elem,
                    sharp: s >= 1.,
                }
            };
            push(elem);
            Some(*background)
        }
    }
}

/// A texture of a privacy look; sharp ones are drawn texel by texel (nearest.frag).
#[derive(Debug, Clone)]
pub struct PrivacyLookElement {
    inner: TextureRenderElement<GlesTexture>,
    sharp: bool,
}

impl PrivacyLookElement {
    fn uniforms(&self) -> Vec<Uniform<'static>> {
        let size = self.inner.buffer().texture().size();
        vec![Uniform::new("tex_size", (size.w as f32, size.h as f32))]
    }

    fn draw_gles(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let program = if self.sharp {
            Shaders::get_from_frame(frame).nearest.clone()
        } else {
            None
        };
        if let Some(program) = &program {
            frame.override_default_tex_program(program.clone(), self.uniforms());
        }
        let res = RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        );
        if program.is_some() {
            frame.clear_tex_program_override();
        }
        res
    }
}

impl Element for PrivacyLookElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for PrivacyLookElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        self.draw_gles(frame, src, dst, damage, opaque_regions, cache)
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for PrivacyLookElement {
    fn draw(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        self.draw_gles(
            frame.as_gles_frame(),
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )?;
        Ok(())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
