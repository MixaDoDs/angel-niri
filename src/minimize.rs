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
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use niri_ipc::Event;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::reexports::calloop::channel;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::utils::{Logical, Rectangle, Scale, Transform};

use crate::layout::workspace::WorkspaceId;
use crate::layout::LayoutElement as _;
use crate::niri::State;
use crate::render_helpers::render_to_vec;
use crate::render_helpers::{RenderCtx, RenderTarget};
use crate::utils::transaction::Transaction;
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
            let saved = (|| -> anyhow::Result<()> {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let mut file = std::fs::File::create(&path)?;
                write_bmp_rgba8(&mut file, w, h, &pixels)?;
                Ok(())
            })();
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

impl State {
    /// `snapshot-window`: the window as it looks now, saved to `path` in a thread.
    pub fn snapshot_window(&mut self, id: u64, path: PathBuf) {
        let Some((Some(mon), mapped)) =
            self.niri.layout.windows().find(|(_, m)| m.id().get() == id)
        else {
            return;
        };
        let output = mon.output().clone();
        let mut pixels = None;
        self.backend.with_primary_renderer(|renderer| {
            match render_window(renderer, &output, mapped) {
                Ok(x) => pixels = Some(x),
                Err(err) => warn!("error snapshotting a window: {err:?}"),
            }
        });
        let Some((w, h, pixels)) = pixels else {
            return;
        };
        thread::spawn(move || {
            let mut buf = vec![];
            let saved = write_png_rgba8(std::io::Cursor::new(&mut buf), w, h, &pixels)
                .map_err(anyhow::Error::from)
                .and_then(|()| {
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    // written next to it and moved over, so a reader never sees half a file
                    let tmp = path.with_extension("part");
                    std::fs::write(&tmp, &buf)?;
                    std::fs::rename(&tmp, &path)?;
                    Ok(())
                });
            if let Err(err) = saved {
                warn!("error saving a window snapshot: {err:?}");
            }
        });
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
    dir.join("niri-minimize").join(format!("{id}-{stamp}.bmp"))
}

/// Writes an uncompressed top-down 32-bit RGBA BMP (`BITMAPV4HEADER` with `BI_BITFIELDS`).
///
/// Minimize snapshots live briefly in `XDG_RUNTIME_DIR` (tmpfs in RAM); skipping zlib PNG
/// compression cuts 1080p snapshot latency from ~200 ms to ~2 ms and speeds up shell decoding.
fn write_bmp_rgba8<W: std::io::Write>(
    mut writer: W,
    w: u32,
    h: u32,
    pixels: &[u8],
) -> anyhow::Result<()> {
    const FILE_HEADER_SIZE: u32 = 14;
    const V4_HEADER_SIZE: u32 = 108;
    const HEADER_SIZE: u32 = FILE_HEADER_SIZE + V4_HEADER_SIZE;

    let expected = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("snapshot dimensions overflow"))?;
    anyhow::ensure!(
        w > 0 && h > 0 && pixels.len() == expected,
        "invalid snapshot buffer size"
    );

    let width = i32::try_from(w)?;
    let neg_height = i32::try_from(h)?
        .checked_neg()
        .ok_or_else(|| anyhow::anyhow!("snapshot height overflow"))?;
    let pixel_bytes = u32::try_from(pixels.len())?;
    let file_size = HEADER_SIZE
        .checked_add(pixel_bytes)
        .ok_or_else(|| anyhow::anyhow!("snapshot file size overflow"))?;

    let mut hdr = [0u8; HEADER_SIZE as usize];
    // BITMAPFILEHEADER (14 bytes)
    hdr[0..2].copy_from_slice(b"BM");
    hdr[2..6].copy_from_slice(&file_size.to_le_bytes());
    hdr[10..14].copy_from_slice(&HEADER_SIZE.to_le_bytes());
    // BITMAPV4HEADER (108 bytes)
    hdr[14..18].copy_from_slice(&V4_HEADER_SIZE.to_le_bytes());
    hdr[18..22].copy_from_slice(&width.to_le_bytes());
    hdr[22..26].copy_from_slice(&neg_height.to_le_bytes()); // negative = top-down
    hdr[26..28].copy_from_slice(&1u16.to_le_bytes()); // planes
    hdr[28..30].copy_from_slice(&32u16.to_le_bytes()); // bpp
    hdr[30..34].copy_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
    hdr[34..38].copy_from_slice(&pixel_bytes.to_le_bytes());
    hdr[38..42].copy_from_slice(&2835i32.to_le_bytes()); // 72 DPI
    hdr[42..46].copy_from_slice(&2835i32.to_le_bytes());
    // RGBA8 channel masks (little-endian: byte 0 = R, 1 = G, 2 = B, 3 = A)
    hdr[54..58].copy_from_slice(&0x0000_00FFu32.to_le_bytes());
    hdr[58..62].copy_from_slice(&0x0000_FF00u32.to_le_bytes());
    hdr[62..66].copy_from_slice(&0x00FF_0000u32.to_le_bytes());
    hdr[66..70].copy_from_slice(&0xFF00_0000u32.to_le_bytes());
    hdr[70..74].copy_from_slice(b"BGRs"); // LCS_sRGB

    writer.write_all(&hdr)?;
    writer.write_all(pixels)?;
    Ok(())
}

static FADE: AtomicBool = AtomicBool::new(false);

/// `set-minimize-fade`: whether a minimized window plays the close animation.
pub fn set_fade(on: bool) {
    FADE.store(on, Ordering::Relaxed);
}

fn fade() -> bool {
    FADE.load(Ordering::Relaxed)
}

static ON_COPY: AtomicBool = AtomicBool::new(true);
static COPIED: Mutex<Option<Instant>> = Mutex::new(None);
/// How soon after a copy a program's own minimizing counts as "minimize on copy".
const AFTER_COPY: Duration = Duration::from_secs(1);

/// `set-minimize-on-copy`: whether a program may minimize itself right after it copies.
pub fn set_on_copy(on: bool) {
    ON_COPY.store(on, Ordering::Relaxed);
}

/// A new clipboard was set.
pub fn copied() {
    *COPIED.lock().unwrap() = Some(Instant::now());
}

impl State {
    /// Whether a program's own request to minimize `window` is followed: not when it is the
    /// focused one and the clipboard has just changed, with `set-minimize-on-copy --on false`.
    pub fn self_minimize_allowed(&self, window: &Window) -> bool {
        if ON_COPY.load(Ordering::Relaxed) {
            return true;
        }
        let focused = self
            .niri
            .layout
            .focus()
            .is_some_and(|m| &m.window == window);
        let just_copied = COPIED
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() < AFTER_COPY);
        !(focused && just_copied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmp_rgba8_header_and_payload() {
        let pixels = [0x12, 0x34, 0x56, 0x80, 0xaa, 0xbb, 0xcc, 0xff];
        let mut out = Vec::new();
        write_bmp_rgba8(&mut out, 2, 1, &pixels).unwrap();

        assert_eq!(out.len(), 122 + pixels.len());
        assert_eq!(&out[0..2], b"BM");
        assert_eq!(u32::from_le_bytes(out[2..6].try_into().unwrap()), 130);
        assert_eq!(u32::from_le_bytes(out[10..14].try_into().unwrap()), 122);
        assert_eq!(i32::from_le_bytes(out[18..22].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(out[22..26].try_into().unwrap()), -1);
        assert_eq!(u32::from_le_bytes(out[30..34].try_into().unwrap()), 3);
        assert_eq!(
            u32::from_le_bytes(out[54..58].try_into().unwrap()),
            0x0000_00FF
        );
        assert_eq!(
            u32::from_le_bytes(out[66..70].try_into().unwrap()),
            0xFF00_0000
        );
        assert_eq!(&out[122..], &pixels);
    }

    #[test]
    fn bmp_rgba8_rejects_mismatched_buffer() {
        let mut out = Vec::new();
        assert!(write_bmp_rgba8(&mut out, 2, 2, &[0; 4]).is_err());
        assert!(write_bmp_rgba8(&mut out, 0, 2, &[]).is_err());
    }
}
