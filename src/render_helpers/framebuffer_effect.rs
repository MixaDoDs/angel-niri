use std::cell::RefCell;
use std::sync::Once;

use glam::{Mat3, Vec2};
use niri_config::CornerRadius;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id, RenderElement};
use smithay::backend::renderer::gles::{
    ffi, Capability, GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform,
};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Frame as _, FrameContext, Offscreen, Texture as _};
use smithay::gpu_span_location;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Transform};

use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};
use crate::render_helpers::background_effect::RenderParams;
use crate::render_helpers::blur::{Blur, BlurOptions};
use crate::render_helpers::renderer::AsGlesFrame as _;
use crate::render_helpers::shaders::{mat3_uniform, Shaders};
use crate::utils::region::TransformedRegion;

#[derive(Debug)]
pub struct FramebufferEffect {
    id: Id,
    commit: CommitCounter,
}

#[derive(Debug)]
pub struct FramebufferEffectElement {
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<f64, Logical>,
    clip_geo: Rectangle<f64, Logical>,
    corner_radius: CornerRadius,
    subregion: Option<TransformedRegion>,
    scale: f32,
    blur_options: Option<BlurOptions>,
    noise: f32,
    saturation: f32,
    /// angel-niri (desk_throw.rs): drawn broken instead (shaders/glitch.frag).
    glitch: Option<crate::desk_throw::GlitchFrame>,
}

#[derive(Debug)]
struct Inner {
    framebuffer: Option<GlesTexture>,
    blur: Option<Blur>,
    intermediate: Option<GlesTexture>,
    /// Reusable storage for subregion-filtered damage rects.
    subregion_damage: Vec<Rectangle<i32, Physical>>,
    /// angel-niri (desk_throw.rs): the frame as each hit came, around the hit (the window pressed
    /// into the edge), for its stuck pieces; and which hit it last kept.
    snap: Option<GlesTexture>,
    snap_key: Option<u64>,
}

impl FramebufferEffect {
    pub fn new() -> Self {
        Self {
            id: Id::new(),
            commit: CommitCounter::default(),
        }
    }

    pub fn damage(&mut self) {
        self.commit.increment();
    }

    pub fn render(
        &self,
        ns: Option<usize>,
        params: RenderParams,
        blur_options: Option<BlurOptions>,
        noise: f32,
        saturation: f32,
    ) -> FramebufferEffectElement {
        let (clip_geo, corner_radius) = params
            .clip
            .unwrap_or((params.geometry, CornerRadius::default()));

        let mut id = self.id.clone();
        if let Some(ns) = ns {
            id = id.namespaced(ns);
        }

        FramebufferEffectElement {
            id,
            commit: self.commit,
            geometry: params.geometry,
            clip_geo,
            corner_radius,
            subregion: params.subregion,
            scale: params.scale as f32,
            blur_options,
            noise,
            saturation,
            glitch: None,
        }
    }

    /// angel-niri (desk_throw.rs): the frame under `geometry` (a whole output, logical), broken
    /// as `glitch` says; damaged every time, it changes every frame.
    pub fn render_glitch(
        &mut self,
        geometry: Rectangle<f64, Logical>,
        scale: f64,
        glitch: crate::desk_throw::GlitchFrame,
    ) -> FramebufferEffectElement {
        self.commit.increment();
        FramebufferEffectElement {
            id: self.id.clone(),
            commit: self.commit,
            geometry,
            clip_geo: geometry,
            corner_radius: CornerRadius::default(),
            subregion: None,
            scale: scale as f32,
            blur_options: None,
            noise: 0.,
            saturation: 1.,
            glitch: Some(glitch),
        }
    }
}

impl FramebufferEffectElement {
    fn compute_uniforms(
        &self,
        crop: Rectangle<f64, Logical>,
        transform: Transform,
    ) -> [Uniform<'static>; 7] {
        let offset = crop.loc - (self.clip_geo.loc - self.geometry.loc);
        let offset = Vec2::new(offset.x as f32, offset.y as f32);
        let crop_size = Vec2::new(crop.size.w as f32, crop.size.h as f32);
        let clip_size = Vec2::new(self.clip_geo.size.w as f32, self.clip_geo.size.h as f32);

        // Our v_coords are [0, 1] inside crop. We want them to be [0, 1] inside clip_geo.
        let input_to_clip_geo =
            Mat3::from_scale(crop_size / clip_size) * Mat3::from_translation(offset / crop_size);

        // Revert the effect of the texture transform.
        let transform_mat = Mat3::from_translation(Vec2::new(0.5, 0.5))
            * transform.matrix()
            * Mat3::from_translation(Vec2::new(-0.5, -0.5));
        let input_to_clip_geo = input_to_clip_geo * transform_mat;

        let clip_geo_size = (self.clip_geo.size.w as f32, self.clip_geo.size.h as f32);

        [
            Uniform::new("niri_scale", self.scale),
            Uniform::new("geo_size", clip_geo_size),
            Uniform::new("corner_radius", <[f32; 4]>::from(self.corner_radius)),
            mat3_uniform("input_to_geo", input_to_clip_geo),
            Uniform::new("noise", self.noise),
            Uniform::new("saturation", self.saturation),
            Uniform::new("bg_color", [0f32, 0., 0., 0.]),
        ]
    }
}

impl Element for FramebufferEffectElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        // We don't use src for drawing but we can use it to figure out how we were cropped.
        let size = self.geometry.size.to_buffer(1., Transform::Normal);
        Rectangle::from_size(size)
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry.to_physical_precise_round(scale)
    }

    fn is_framebuffer_effect(&self) -> bool {
        true
    }
}

impl RenderElement<GlesRenderer> for FramebufferEffectElement {
    fn capture_framebuffer(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        let _span = tracy_client::span!("FramebufferEffectElement::capture_framebuffer");
        let location = gpu_span_location!("FramebufferEffectElement::capture_framebuffer");
        frame.with_gpu_span(location, |frame| {
            let output_rect = Rectangle::from_size(frame.output_size());
            let transform = frame.transformation();

            let supports_blit = frame.capabilities().contains(&Capability::Blit);
            let mut guard = frame.renderer();

            let inner = cache
                .get_or_insert::<RefCell<Inner>, _>(|| RefCell::new(Inner::new(guard.as_mut())));
            let mut inner = inner.borrow_mut();
            let inner = &mut *inner;

            inner.intermediate = None;

            if !supports_blit {
                static WARN: Once = Once::new();
                WARN.call_once(|| {
                    warn!("non-xray background effects disabled: renderer does not support framebuffer blitting");
                });
                return Ok(());
            }

            // We want clamp-to-edge behavior for out-of-bounds pixels. However, glBlitFramebuffer
            // seems to skip out-of-bounds pixels, even though my reading of the docs suggests
            // otherwise (we use GL_LINEAR filter). So, clamp dst to the framebuffer bounds
            // ourselves.
            let clamped_dst = match dst.intersection(output_rect) {
                Some(clamped) => clamped,
                None => return Ok(()),
            };
            let clamp_scale = clamped_dst.size.to_f64() / dst.size.to_f64();

            let dst = transform.transform_rect_in(clamped_dst, &output_rect.size);

            // Compute size from our geometry and scale.
            //
            // The "correct" size is always dst.size since that's the pixel region we're actually
            // blitting. However, using dst.size causes two undesirable things when zooming out for
            // the overview:
            // 1. dst.size shrinks every frame, causing a texture realloaction for every fb effect
            //    element every frame.
            // 2. The underlying blur visually expands. This is technically correct, since the
            //    underlying contents shrink, but it's not what you visually expect: you expect the
            //    blur to also shrink as the windows zoom out, to give the zooming out effect.
            //
            // Using size computed from geometry and scale solves both of those problems (even
            // though there's a bit of a cost in that zoomed-out elements still blur the entire
            // unzoomed texture size, and even though the blur ends up slightly wrong as there's two
            // layers of texture resampling, up and back down).
            //
            // Here we use src.size rather than geometry directly because src takes into account
            // cropping.
            let size = src
                .size
                .to_logical(1., Transform::Normal)
                .upscale(clamp_scale)
                .to_physical_precise_round(self.scale);
            let size = transform.transform_size(size);

            let size = size.to_logical(1).to_buffer(1, Transform::Normal);

            // Recreate framebuffer if needed.
            if inner
                .framebuffer
                .as_ref()
                .is_some_and(|fb| fb.size() != size)
            {
                inner.framebuffer = None;
            }
            let framebuffer = if let Some(fb) = &inner.framebuffer {
                fb
            } else {
                trace!("creating framebuffer texture sized {} × {}", size.w, size.h);
                let renderer = guard.as_mut();
                let texture = renderer.create_buffer(Fourcc::Abgr8888, size)?;
                inner.framebuffer.insert(texture)
            };

            // Prepare blur textures.
            let mut blur = Option::zip(inner.blur.as_mut(), self.blur_options);
            if let Some((b, options)) = &mut blur {
                let renderer = guard.as_mut();
                if let Err(err) = b.prepare_textures(
                    |fourcc, size| renderer.create_buffer(fourcc, size),
                    framebuffer,
                    *options,
                ) {
                    warn!("error preparing blur textures: {err:?}");
                    blur = None;
                }
            }

            // angel-niri (desk_throw.rs): a new hit: keep the frame around it
            let snap_hit = (self.glitch.and_then(|g| g.snap)).filter(|(key, _)| inner.snap_key != Some(*key));
            if snap_hit.is_some() && inner.snap.as_ref().is_none_or(|snap| snap.size() != size) {
                let renderer = guard.as_mut();
                inner.snap = Some(renderer.create_buffer(Fourcc::Abgr8888, size)?);
                // the first one keeps all of it, so nothing undefined shows
                inner.snap_key = None;
            }

            // We can't use renderer.with_context() as that will reset the GlesFrame binding that we
            // want to blit from.
            drop(guard);

            // Blit the framebuffer contents.
            frame.with_context(|gl| unsafe {
                while gl.GetError() != ffi::NO_ERROR {}

                let mut current_fbo = 0i32;
                gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut current_fbo as *mut _);

                // BlitFramebuffer is affected by the scissor test, we don't want that.
                gl.Disable(ffi::SCISSOR_TEST);

                let mut fbo = 0;
                gl.GenFramebuffers(1, &mut fbo as *mut _);
                gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);

                gl.FramebufferTexture2D(
                    ffi::DRAW_FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::TEXTURE_2D,
                    framebuffer.tex_id(),
                    0,
                );

                gl.BlitFramebuffer(
                    dst.loc.x,
                    dst.loc.y,
                    dst.loc.x + dst.size.w,
                    dst.loc.y + dst.size.h,
                    0,
                    0,
                    size.w,
                    size.h,
                    ffi::COLOR_BUFFER_BIT,
                    ffi::LINEAR,
                );

                // Restore state set by GlesFrame that we just modified.
                gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, current_fbo as u32);
                gl.Enable(ffi::SCISSOR_TEST);

                gl.DeleteFramebuffers(1, &mut fbo as *mut _);

                if gl.GetError() != ffi::NO_ERROR {
                    Err(GlesError::BlitError)
                } else {
                    Ok(())
                }
            })??;

            if let (Some((key, pos)), Some(snap)) = (snap_hit, &inner.snap) {
                let full = Rectangle::from_size(size);
                let area = if inner.snap_key.is_none() {
                    full
                } else {
                    // around the hit, down from the edge: where the window that hit is now
                    let scale = f64::from(self.scale);
                    let screen = (self.geometry.size.w * scale, self.geometry.size.h * scale);
                    let tex_to_screen = Mat3::from_translation(Vec2::new(0.5, 0.5))
                        * transform.matrix()
                        * Mat3::from_translation(Vec2::new(-0.5, -0.5));
                    let to_tex = tex_to_screen.inverse();
                    let (x, y) = (f64::from(pos[0]) * scale, f64::from(pos[1]) * scale);
                    let (w, h) = (420. * scale, 460. * scale);
                    let corner = |sx: f64, sy: f64| {
                        let p = to_tex.transform_point2(Vec2::new(
                            (sx / screen.0) as f32,
                            (sy / screen.1) as f32,
                        ));
                        (f64::from(p.x) * f64::from(size.w), f64::from(p.y) * f64::from(size.h))
                    };
                    let a = corner(x - w, y - h);
                    let b = corner(x + w, y + h);
                    let lo = (a.0.min(b.0).floor() as i32, a.1.min(b.1).floor() as i32);
                    let hi = (a.0.max(b.0).ceil() as i32, a.1.max(b.1).ceil() as i32);
                    Rectangle::from_extremities(lo, hi)
                        .intersection(full)
                        .unwrap_or_default()
                };
                let (from, into) = (framebuffer.tex_id(), snap.tex_id());
                frame.with_context(|gl| unsafe {
                    while gl.GetError() != ffi::NO_ERROR {}
                    let mut read_fbo = 0i32;
                    let mut draw_fbo = 0i32;
                    gl.GetIntegerv(ffi::READ_FRAMEBUFFER_BINDING, &mut read_fbo as *mut _);
                    gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut draw_fbo as *mut _);
                    gl.Disable(ffi::SCISSOR_TEST);
                    let mut fbos = [0u32; 2];
                    gl.GenFramebuffers(2, fbos.as_mut_ptr());
                    gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, fbos[0]);
                    gl.FramebufferTexture2D(
                        ffi::READ_FRAMEBUFFER,
                        ffi::COLOR_ATTACHMENT0,
                        ffi::TEXTURE_2D,
                        from,
                        0,
                    );
                    gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbos[1]);
                    gl.FramebufferTexture2D(
                        ffi::DRAW_FRAMEBUFFER,
                        ffi::COLOR_ATTACHMENT0,
                        ffi::TEXTURE_2D,
                        into,
                        0,
                    );
                    let (x0, y0) = (area.loc.x, area.loc.y);
                    let (x1, y1) = (x0 + area.size.w, y0 + area.size.h);
                    gl.BlitFramebuffer(
                        x0,
                        y0,
                        x1,
                        y1,
                        x0,
                        y0,
                        x1,
                        y1,
                        ffi::COLOR_BUFFER_BIT,
                        ffi::NEAREST,
                    );
                    gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, read_fbo as u32);
                    gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, draw_fbo as u32);
                    gl.Enable(ffi::SCISSOR_TEST);
                    gl.DeleteFramebuffers(2, fbos.as_ptr());
                    if gl.GetError() != ffi::NO_ERROR {
                        warn!("error keeping the frame of a hit");
                    }
                })?;
                inner.snap_key = Some(key);
            }

            // If blur is off, use the unblurred texture.
            if self.blur_options.is_none() {
                inner.intermediate = Some(framebuffer.clone());
                return Ok(());
            }

            if let Some((blur, options)) = blur {
                let mut guard = frame.renderer();
                let renderer = guard.as_mut();
                match blur.render(renderer, framebuffer, options) {
                    Ok(blurred) => inner.intermediate = Some(blurred),
                    Err(err) => {
                        warn!("error rendering blur: {err:?}");
                    }
                }
            }

            Ok(())
        })
    }

    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let Some(cache) = cache else {
            return Ok(());
        };
        let Some(inner) = cache.get::<RefCell<Inner>>() else {
            return Ok(());
        };
        let mut inner = inner.borrow_mut();
        let inner = &mut *inner;

        let Some(texture) = &inner.intermediate else {
            return Ok(());
        };

        // Clamp the same way as in capture_framebuffer().
        let output_rect = Rectangle::from_size(frame.output_size());
        let clamped_dst = match dst.intersection(output_rect) {
            Some(clamped) => clamped,
            None => return Ok(()),
        };
        let clamp_offset = clamped_dst.loc - dst.loc;

        // Filter damage by subregion, reusing the stored Vec to avoid allocation.
        let filtered = &mut inner.subregion_damage;
        filtered.clear();

        if let Some(subregion) = &self.subregion {
            // Convert to subregion coordinates.
            let mut crop = src.to_logical(1., Transform::Normal, &src.size);
            crop.loc += self.geometry.loc;
            subregion.filter_damage(crop, dst, damage, filtered);
        } else {
            filtered.extend(damage.iter());
        };

        // Adjust for clamped dst.
        if clamped_dst != dst {
            let r = Rectangle::new(clamp_offset, clamped_dst.size);
            filtered.retain_mut(|d| {
                if let Some(mut crop) = d.intersection(r) {
                    crop.loc -= clamp_offset;
                    *d = crop;
                    true
                } else {
                    false
                }
            });
        }

        if filtered.is_empty() {
            return Ok(());
        }
        let damage = &filtered[..];

        // Adjust src proportionally to the dst clamping.
        let src_loc = src.loc.to_logical(1., Transform::Normal, &src.size);
        let dst_to_src = src.size / dst.size.to_f64();
        let crop = Rectangle::new(
            src_loc + clamp_offset.to_f64().upscale(dst_to_src).to_logical(1.),
            clamped_dst.size.to_f64().upscale(dst_to_src).to_logical(1.),
        );

        if let Some(glitch) = self.glitch {
            let program = Shaders::get_from_frame(frame).glitch.clone();
            let size = texture.size();
            let scale = f64::from(self.scale);
            // the damage is laid out on the upright screen in physical pixels; the captured
            // frame's texels are that turned by the output's transform (the winit window's frame
            // is upside down, a monitor's isn't)
            let screen = [
                (self.geometry.size.w * scale) as f32,
                (self.geometry.size.h * scale) as f32,
            ];
            let tex_to_screen = Mat3::from_translation(Vec2::new(0.5, 0.5))
                * frame.transformation().matrix()
                * Mat3::from_translation(Vec2::new(-0.5, -0.5));
            let spot = |i: usize| {
                let [x, y, age, seed] = glitch.spots[i];
                [x * self.scale, y * self.scale, age, seed]
            };
            // its stuck pieces come from the frames kept at the hits, on the second texture unit
            let snap = inner.snap.as_ref().map(|snap| snap.tex_id());
            if let Some(snap) = snap {
                frame.with_context(|gl| unsafe {
                    gl.ActiveTexture(ffi::TEXTURE1);
                    gl.BindTexture(ffi::TEXTURE_2D, snap);
                    gl.ActiveTexture(ffi::TEXTURE0);
                })?;
            }
            let uniforms = [
                Uniform::new("g_snap", 1i32),
                Uniform::new("g_snap_on", if snap.is_some() { 1f32 } else { 0. }),
                Uniform::new("tex_size", [size.w as f32, size.h as f32]),
                Uniform::new("g_screen", screen),
                mat3_uniform("g_tex_to_screen", tex_to_screen),
                mat3_uniform("g_screen_to_tex", tex_to_screen.inverse()),
                Uniform::new("g_h0", spot(0)),
                Uniform::new("g_h1", spot(1)),
                Uniform::new("g_h2", spot(2)),
                Uniform::new("g_h3", spot(3)),
                Uniform::new("g_h4", spot(4)),
                Uniform::new("g_h5", spot(5)),
                Uniform::new(
                    "g_wa",
                    [glitch.swells[0], glitch.swells[1], glitch.swells[2]],
                ),
                Uniform::new(
                    "g_wb",
                    [glitch.swells[3], glitch.swells[4], glitch.swells[5]],
                ),
                Uniform::new("g_jolt", [glitch.jolt[0], glitch.jolt[1] * self.scale]),
                Uniform::new("g_step", glitch.step),
                Uniform::new("g_px", self.scale),
                Uniform::new("g_grow", glitch.grow),
                Uniform::new("g_left", glitch.left),
            ];
            let drawn = frame.render_texture_from_to(
                texture,
                Rectangle::from_size(texture.size().to_f64()),
                clamped_dst,
                damage,
                &[],
                frame.transformation().invert(),
                1.,
                program.as_ref(),
                if program.is_some() {
                    &uniforms[..]
                } else {
                    &[]
                },
            );
            if snap.is_some() {
                frame.with_context(|gl| unsafe {
                    gl.ActiveTexture(ffi::TEXTURE1);
                    gl.BindTexture(ffi::TEXTURE_2D, 0);
                    gl.ActiveTexture(ffi::TEXTURE0);
                })?;
            }
            return drawn;
        }

        let program = Shaders::get_from_frame(frame).postprocess_and_clip.clone();
        let uniforms = program
            .is_some()
            .then(|| self.compute_uniforms(crop, frame.transformation()));
        let uniforms = uniforms.as_ref().map_or(&[][..], |x| &x[..]);

        frame.render_texture_from_to(
            texture,
            Rectangle::from_size(texture.size().to_f64()),
            clamped_dst,
            damage,
            &[],
            // The intermediate texture has the same transform as the frame.
            frame.transformation().invert(),
            1.,
            program.as_ref(),
            uniforms,
        )
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for FramebufferEffectElement {
    fn capture_framebuffer(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), TtyRendererError<'render>> {
        let gles_frame = frame.as_gles_frame();
        RenderElement::<GlesRenderer>::capture_framebuffer(&self, gles_frame, src, dst, cache)?;
        Ok(())
    }

    fn draw(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        let gles_frame = frame.as_gles_frame();
        RenderElement::<GlesRenderer>::draw(
            &self,
            gles_frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )?;
        Ok(())
    }
}

impl Inner {
    fn new(renderer: &mut GlesRenderer) -> Self {
        Inner {
            framebuffer: None,
            blur: Blur::new(renderer),
            intermediate: None,
            subregion_damage: Vec::new(),
            snap: None,
            snap_key: None,
        }
    }
}
