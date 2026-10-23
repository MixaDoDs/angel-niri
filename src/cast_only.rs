//! angel-niri: layers for the viewers. `niri msg action set-cast-only-layer --namespace NS
//! [--on false]`: layer-shell surfaces of that namespace are drawn only into screencasts (what OBS
//! records through the portal), not on the monitor, in screenshots or screencopy. A shell puts
//! its stream overlays (chat, plates) there, so the stream shows them and the streamer doesn't.
//! Nothing goes to niri's config.
//!
//! shortcut: input is not filtered, so such a surface should keep an empty input region and no
//! keyboard interactivity; filter it in the hit tests if a shell ever needs more.

use std::sync::Mutex;

use crate::render_helpers::RenderTarget;

static NAMESPACES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn set(namespace: String, on: bool) {
    let mut namespaces = NAMESPACES.lock().unwrap();
    namespaces.retain(|ns| *ns != namespace);
    if on {
        namespaces.push(namespace);
    }
}

/// Whether layer surfaces of this namespace are drawn only into screencasts.
pub fn is_cast_only(namespace: &str) -> bool {
    NAMESPACES.lock().unwrap().iter().any(|ns| ns == namespace)
}

/// Whether a layer surface of this namespace stays out of what is rendered for `target`.
pub fn hides(namespace: &str, target: RenderTarget) -> bool {
    target != RenderTarget::Screencast && is_cast_only(namespace)
}
