//! angel-niri: minimizing windows.
//!
//! The layout keeps minimized windows out of the workspaces (`Layout::minimize_window()`). Here is
//! how a window gets there. An app's own minimize button, a taskbar, a bind or
//! `niri msg action minimize-window` *ask*: niri snapshots the window as it is on screen, and when
//! a shell listens on the event stream it sends `WindowMinimizeRequested` with the snapshot and
//! where the window stands, so the shell can put the snapshot over the window and animate it into
//! its dock. The shell then minimizes it with `minimize-window --now`. Nobody listening, or no
//! answer in `HAND_OFF`: niri minimizes it itself.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use niri_ipc::Event;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::reexports::calloop::channel;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::utils::{Logical, Rectangle, Scale, Transform};

use crate::layout::workspace::WorkspaceId;
use crate::utils::transaction::Transaction;
use crate::layout::LayoutElement as _;
use crate::niri::State;
use crate::render_helpers::render_to_vec;
use crate::render_helpers::{RenderCtx, RenderTarget};
use crate::utils::write_png_rgba8;
use crate::window::Mapped;

/// How long a shell gets to take a window it was told about.
const HAND_OFF: Duration = Duration::from_millis(600);

impl State {
    /// Asks for the window to be minimized (see the module docs).
    pub fn request_minimize(&mut self, window: &Window) {
        let Some(mapped) = self
            .niri
            .layout
            .windows()
            .find(|(_, m)| &m.window == window)
        else {
            return;
        };
        let mapped = mapped.1;
        let id = mapped.id().get();
        if self.niri.minimize_pending.contains(&id) {
            return;
        }

        let listened = self
            .niri
            .ipc_server
            .as_ref()
            .is_some_and(|server| server.has_event_streams());
        if !listened {
            self.minimize_window(window);
            return;
        }

        let rect = self
            .niri
            .layout
            .window_render_rect(window)
            .map(|(output, rect)| (output.clone(), rect));

        let mut pixels = None;
        if let Some((output, _)) = &rect {
            self.backend.with_primary_renderer(|renderer| {
                match render_window(renderer, output, mapped) {
                    Ok(x) => pixels = Some(x),
                    Err(err) => warn!("error snapshotting a window to minimize: {err:?}"),
                }
            });
        }

        self.niri.minimize_pending.insert(id);

        let output_name = rect.as_ref().map(|(output, _)| output.name());
        let rect = rect.map(|(_, rect)| rect);

        let Some((w, h, pixels)) = pixels else {
            self.minimize_requested(id, output_name, rect, None);
            return;
        };

        // Encode and save in a thread, as screenshots do, then tell the shell.
        let (tx, rx) = channel::sync_channel::<Option<String>>(1);
        self.niri
            .event_loop
            .insert_source(rx, move |event, _, state| {
                if let channel::Event::Msg(path) = event {
                    state.minimize_requested(id, output_name.clone(), rect, path);
                }
            })
            .unwrap();
        let path = snapshot_path(id);
        thread::spawn(move || {
            let mut buf = vec![];
            let saved = write_png_rgba8(std::io::Cursor::new(&mut buf), w, h, &pixels)
                .map_err(anyhow::Error::from)
                .and_then(|()| {
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    std::fs::write(&path, &buf)?;
                    Ok(())
                });
            let path = match saved {
                Ok(()) => path.to_str().map(String::from),
                Err(err) => {
                    warn!("error saving a minimize snapshot: {err:?}");
                    None
                }
            };
            let _ = tx.send(path);
        });
    }

    /// The snapshot is ready (or there is none): tell the shell, and minimize it ourselves if the
    /// shell does not take it in time.
    fn minimize_requested(
        &mut self,
        id: u64,
        output: Option<String>,
        rect: Option<Rectangle<f64, Logical>>,
        snapshot: Option<String>,
    ) {
        if !self.niri.minimize_pending.contains(&id) {
            return;
        }

        if let Some(server) = &self.niri.ipc_server {
            server.send_transient_event(Event::WindowMinimizeRequested {
                id,
                output,
                rect: rect.map(|r| [r.loc.x, r.loc.y, r.size.w, r.size.h]),
                snapshot,
            });
        }

        self.niri
            .event_loop
            .insert_source(Timer::from_duration(HAND_OFF), move |_, _, state| {
                if state.niri.minimize_pending.remove(&id) {
                    let window = state
                        .niri
                        .layout
                        .windows()
                        .find(|(_, m)| m.id().get() == id);
                    if let Some(window) = window.map(|(_, m)| m.window.clone()) {
                        state.minimize_window(&window);
                    }
                }
                TimeoutAction::Drop
            })
            .unwrap();
    }

    /// Takes a window out of the layout now.
    pub fn minimize_window(&mut self, window: &Window) {
        let was_active = self.niri.layout.focus().map(|m| &m.window) == Some(window);

        // `set-minimize-fade`: it goes the way a closing window does
        if fade() {
            let output = self
                .niri
                .layout
                .windows()
                .find(|(_, m)| &m.window == window)
                .and_then(|(mon, _)| mon.map(|mon| mon.output().clone()));
            if output.is_some() {
                self.store_unmap_snapshot(window, output.as_ref());
                let transaction = Transaction::new();
                let blocker = transaction.blocker();
                self.backend.with_primary_renderer(|renderer| {
                    self.niri
                        .layout
                        .start_close_animation_for_window(renderer, window, blocker);
                });
            }
        }

        if let Some(mapped) = self
            .niri
            .layout
            .windows()
            .find(|(_, m)| &m.window == window)
        {
            self.niri.minimize_pending.remove(&mapped.1.id().get());
        }

        if !self.niri.layout.minimize_window(window) {
            return;
        }

        if was_active {
            self.maybe_warp_cursor_to_focus();
        }

        // FIXME: granular
        self.niri.queue_redraw_all();
    }

    /// Puts a minimized window back where it was, or onto workspace `to`. Returns whether it was
    /// minimized.
    pub fn unminimize_window(
        &mut self,
        window: &Window,
        activate: bool,
        to: Option<WorkspaceId>,
    ) -> bool {
        if !self.niri.layout.unminimize_window(window, activate, to) {
            return false;
        }

        self.niri.layout.start_open_animation_for_window(window);

        if activate {
            self.niri.layer_shell_on_demand_focus = None;
            self.focus_window(window);
        } else {
            // FIXME: granular
            self.niri.queue_redraw_all();
        }
        true
    }
}

/// The window as it looks on `output`, cut to its geometry: (width, height, RGBA pixels).
/// Rendered as for a screen capture, so windows blocked out from casts stay blocked out.
fn render_window(
    renderer: &mut GlesRenderer,
    output: &Output,
    mapped: &Mapped,
) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let scale = Scale::from(output.current_scale().fractional_scale());
    let mut elements = Vec::new();
    let ctx = RenderCtx {
        renderer,
        target: RenderTarget::ScreenCapture,
        xray: None,
    };
    mapped.render_normal(ctx, (0., 0.).into(), scale, 1., &mut |elem| {
        elements.push(elem)
    });
    let size = mapped.size().to_f64().to_physical_precise_round(scale);
    let pixels = render_to_vec(
        renderer,
        size,
        scale,
        Transform::Normal,
        Fourcc::Abgr8888,
        elements.iter(),
    )?;
    Ok((size.w as u32, size.h as u32, pixels))
}

fn snapshot_path(id: u64) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    dir.join("niri-minimize").join(format!("{id}-{stamp}.png"))
}

static FADE: AtomicBool = AtomicBool::new(false);

/// `set-minimize-fade`: whether a minimized window plays the close animation.
pub fn set_fade(on: bool) {
    FADE.store(on, Ordering::Relaxed);
}

fn fade() -> bool {
    FADE.load(Ordering::Relaxed)
}
