//! angel-niri: a floating window's desktop flags while stacking — shaded (rolled up into its
//! title bar), on top (above every other window, fullscreen ones too) and sticky (on every
//! workspace of its monitor).
//!
//! The shell sets them (`set-window-flags`, its window menu), the wheel over a title bar shades
//! (up) and unshades (down), a click on a title bar's badge clears on top or sticky, and
//! maximizing a shaded window unrolls it first. Leaving stacking clears them all. Only floating
//! windows while stacking take them; only windows with niri's title bar shade.

use niri_ipc::Event;

use crate::layout::LayoutElement as _;
use crate::niri::State;

impl State {
    /// Sets the flags given on the window (`id`, else the focused one).
    pub fn set_window_flags(
        &mut self,
        id: Option<u64>,
        shaded: Option<bool>,
        on_top: Option<bool>,
        sticky: Option<bool>,
    ) {
        if !crate::stacking::on() {
            return;
        }
        let focused = self.niri.layout.focus().map(|m| m.id().get());
        let Some(id) = id.or(focused) else {
            return;
        };
        self.niri.layout.with_windows_mut(|mapped, _| {
            if mapped.id().get() != id || !mapped.is_floating() {
                return;
            }
            if let Some(on) = shaded {
                mapped.set_shaded(on && mapped.has_ssd());
            }
            if let Some(on) = on_top {
                mapped.set_on_top(on);
            }
            if let Some(on) = sticky {
                mapped.set_sticky(on);
            }
        });
        self.niri.layout.restack_floating();
        self.niri.layout.refresh_sticky();
        self.niri.queue_redraw_all();
    }

    /// Leaving stacking: every window back to normal.
    pub fn clear_window_flags(&mut self) {
        self.niri.layout.with_windows_mut(|mapped, _| {
            mapped.set_shaded(false);
            mapped.set_on_top(false);
            mapped.set_sticky(false);
        });
        self.niri.layout.restack_floating();
    }

    /// Whether the window is shaded; unrolls it if so (before it maximizes).
    pub fn unshade(&mut self, window: &smithay::desktop::Window) -> bool {
        let shaded = (self.niri.layout.windows()).any(|(_, m)| &m.window == window && m.shaded());
        if shaded {
            let id = (self.niri.layout.windows())
                .find(|(_, m)| &m.window == window)
                .map(|(_, m)| m.id().get());
            self.set_window_flags(id, Some(false), None, None);
        }
        shaded
    }

    /// The shell's menu for the window (`id`, else the focused one) at its title bar, as a right
    /// click there asks for it (Alt+Space).
    pub fn request_window_menu(&mut self, id: Option<u64>) {
        if !crate::stacking::on() {
            return;
        }
        let focused = self.niri.layout.focus().map(|m| m.id().get());
        let Some(id) = id.or(focused) else {
            return;
        };
        let Some(window) = (self.niri.layout.windows())
            .find(|(_, m)| m.id().get() == id)
            .map(|(_, m)| m.window.clone())
        else {
            return;
        };
        // the window's top left, just under its title bar, on its output
        let Some((output, rect)) = (self.niri.layout.window_render_rect(&window))
            .map(|(output, rect)| (output.clone(), rect))
        else {
            return;
        };
        let Some(geo) = self.niri.global_space.output_geometry(&output) else {
            return;
        };
        let pos = rect.loc - geo.loc.to_f64();
        if let Some(server) = &self.niri.ipc_server {
            server.send_transient_event(Event::WindowMenuRequested {
                id,
                output: output.name(),
                pos: [pos.x + 8., pos.y],
                green: false,
            });
        }
    }
}
