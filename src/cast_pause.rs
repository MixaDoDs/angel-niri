//! angel-niri: a pause for the viewers. `niri msg action set-cast-pause [--on false] [--image PNG]
//! [--background #rrggbb]`: every screencast (OBS and the rest through the portal, a window's too)
//! shows a card instead of the screen — the picture centred in whole steps and sharp, on the
//! background — and no pointer, while the monitor goes on as it is. Off, the casts are live again.
//! The card doesn't change, so a paused cast sends nothing new. Sound is not niri's: it goes to
//! OBS past it. A shell sets it; nothing goes to niri's config.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context as _;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Size};

use crate::privacy_look::{self, Picture, PrivacyLookElement, WindowLook};
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};

struct Pause {
    picture: Option<Rc<Picture>>,
    background: [f32; 4],
}

thread_local! {
    static PAUSE: RefCell<Option<Pause>> = const { RefCell::new(None) };
    /// Kept between frames, so an unchanged card is no damage.
    static CACHE: RefCell<(WindowLook, SolidColorBuffer)> =
        RefCell::new((WindowLook::default(), SolidColorBuffer::default()));
}

pub fn set(on: bool, image: Option<&str>, background: Option<&str>) -> anyhow::Result<()> {
    let pause = if on {
        let picture = image
            .map(|path| {
                privacy_look::load_png(path).with_context(|| format!("error loading {path}"))
            })
            .transpose()?
            .map(Rc::new);
        let background = match background {
            Some(hex) => {
                privacy_look::parse_color(hex).context("--background is #rrggbb or #rrggbbaa")?
            }
            None => [0., 0., 0., 1.],
        };
        Some(Pause {
            picture,
            background,
        })
    } else {
        None
    };
    PAUSE.with_borrow_mut(|p| *p = pause);
    Ok(())
}

pub enum PauseElement {
    Picture(PrivacyLookElement),
    Fill(SolidColorRenderElement),
}

/// The card over a cast of `size`, front to back; `None` when not paused.
pub fn elements(
    renderer: &mut GlesRenderer,
    size: Size<f64, Logical>,
) -> Option<Vec<PauseElement>> {
    PAUSE.with_borrow(|pause| {
        let pause = pause.as_ref()?;
        CACHE.with_borrow_mut(|(look, fill)| {
            let mut out = Vec::new();
            if let Some(picture) = &pause.picture {
                privacy_look::render_picture(
                    renderer,
                    picture,
                    false,
                    pause.background,
                    look,
                    size,
                    Point::default(),
                    1.,
                    &mut |elem| out.push(PauseElement::Picture(elem)),
                );
            }
            fill.resize(size);
            fill.set_color(pause.background);
            out.push(PauseElement::Fill(SolidColorRenderElement::from_buffer(
                fill,
                Point::default(),
                1.,
                Kind::Unspecified,
            )));
            Some(out)
        })
    })
}
