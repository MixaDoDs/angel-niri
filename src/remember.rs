//! angel-niri: a program opens where it was. `niri msg action set-remember-windows [--on false]`:
//! when a window closes (and for every window when niri quits), its program's place is written to
//! `$XDG_STATE_HOME/angel-niri/windows.json` by app-id — the output, the workspace (by name, else
//! its index on the output), a tiled window's width, a floating one's size and position in the
//! working area. The program's next window opens there with that size. Window rules from the config
//! always win: only what they leave unset is filled in. Dialogs (windows with a parent), fullscreen
//! and maximized sizes are not remembered. Whether it is on lives in the same file, so it works
//! from niri's start, before a shell is up; nothing goes to niri's config.

use std::collections::HashMap;
use std::path::PathBuf;

use niri_config::{FloatOrInt, FloatingPosition, PresetSize, RelativeTo};
use serde::{Deserialize, Serialize};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::layout::workspace::WorkspaceId;
use crate::layout::{Layout, LayoutElement as _};
use crate::niri::Niri;
use crate::utils::with_toplevel_role;
use crate::window::{Mapped, ResolvedWindowRules};

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Spot {
    pub output: String,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub workspace_idx: usize,
    #[serde(default)]
    pub tiled_width: Option<f64>,
    #[serde(default)]
    pub floating_size: Option<(f64, f64)>,
    #[serde(default)]
    pub floating_pos: Option<(f64, f64)>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Remember {
    pub on: bool,
    #[serde(default)]
    pub apps: HashMap<String, Spot>,
    /// Workspaces picked by index for windows not mapped yet.
    #[serde(skip)]
    pub targets: HashMap<WlSurface, WorkspaceId>,
    /// App-ids as of the initial configure: an unmapping window has lost its own by the time
    /// niri sees the commit.
    #[serde(skip)]
    pub app_ids: HashMap<WlSurface, String>,
}

fn path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(dir.join("angel-niri/windows.json"))
}

impl Remember {
    pub fn load() -> Self {
        // (tests never touch the owner's file)
        if cfg!(test) {
            return Self::default();
        }
        path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if cfg!(test) {
            return;
        }
        let Some(p) = path() else { return };
        let Ok(json) = serde_json::to_vec_pretty(self) else {
            return;
        };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // a whole file or the old one: written aside, then renamed over
        let tmp = p.with_extension("json.tmp");
        if let Err(err) = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, &p)) {
            warn!("error saving remembered windows: {err:?}");
        }
    }
}

fn app_id(toplevel: &ToplevelSurface) -> Option<String> {
    with_toplevel_role(toplevel, |role| role.app_id.clone()).filter(|id| !id.is_empty())
}

/// The spot of a mapped window now, or `None` for one not to remember.
fn spot_of(
    layout: &Layout<Mapped>,
    app_ids: &HashMap<WlSurface, String>,
    surface: &WlSurface,
) -> Option<(String, Spot, Spot)> {
    for (mon, idx, ws) in layout.workspaces() {
        let Some(mon) = mon else { continue };
        for (tile, ipc) in ws.tiles_with_ipc_layouts() {
            let window = tile.window();
            if !window.is_wl_surface(surface) {
                continue;
            }
            let toplevel = window.toplevel();
            let id = app_id(toplevel).or_else(|| app_ids.get(surface).cloned())?;
            // dialogs follow their parent; angelOS places its own windows
            if toplevel.parent().is_some() || id.starts_with("org.quickshell") {
                return None;
            }
            let name = ws.name().cloned();
            // angelOS's hidden workspaces (minimized windows)
            if name.as_deref().is_some_and(|n| n.starts_with("angelos-")) {
                return None;
            }
            let place = Spot {
                output: mon.output().name(),
                workspace: name,
                workspace_idx: idx,
                ..Spot::default()
            };
            let mut size = Spot::default();
            let (w, h) = ipc.window_size;
            if window.sizing_mode().is_normal() && w > 0 && h > 0 {
                if let Some((x, y)) = ipc.tile_pos_in_workspace_view {
                    let area = ws.working_area();
                    size.floating_size = Some((f64::from(w), f64::from(h)));
                    size.floating_pos = Some((x - area.loc.x, y - area.loc.y));
                } else {
                    size.tiled_width = Some(f64::from(w));
                }
            }
            return Some((id, place, size));
        }
    }
    None
}

impl Niri {
    pub fn set_remember_windows(&mut self, on: bool) {
        self.remember.on = on;
        if !on {
            self.remember.apps.clear();
        }
        self.remember.save();
    }

    /// Before a window goes away: its program's spot.
    pub fn remember_window(&mut self, surface: &WlSurface) {
        if self.remember_take(surface) {
            self.remember.save();
        }
        self.remember.app_ids.remove(surface);
    }

    fn remember_take(&mut self, surface: &WlSurface) -> bool {
        if !self.remember.on {
            return false;
        }
        let Some((id, place, size)) = spot_of(&self.layout, &self.remember.app_ids, surface) else {
            return false;
        };
        let spot = self.remember.apps.entry(id).or_default();
        let old = spot.clone();
        // the place always; the size of the kind it was (tiled or floating), the other kept
        spot.output = place.output;
        spot.workspace = place.workspace;
        spot.workspace_idx = place.workspace_idx;
        if size.tiled_width.is_some() {
            spot.tiled_width = size.tiled_width;
        }
        if size.floating_size.is_some() {
            spot.floating_size = size.floating_size;
            spot.floating_pos = size.floating_pos;
        }
        *spot != old
    }

    /// When niri quits: every window, the most recently focused last (it wins for its program).
    pub fn remember_all(&mut self) {
        if !self.remember.on {
            return;
        }
        let mut windows: Vec<_> = self
            .layout
            .windows()
            .map(|(_, m)| (m.get_focus_timestamp(), m.toplevel().wl_surface().clone()))
            .collect();
        windows.sort_by_key(|(t, _)| *t);
        let mut changed = false;
        for (_, surface) in windows {
            changed |= self.remember_take(&surface);
        }
        if changed {
            self.remember.save();
        }
    }
}

/// At a new window's initial configure: what the rules leave unset, from its program's spot. A
/// workspace picked by index is returned, to be used when the window maps.
pub fn apply(
    remember: &mut Remember,
    layout: &Layout<Mapped>,
    rules: &mut ResolvedWindowRules,
    toplevel: &ToplevelSurface,
) -> Option<WorkspaceId> {
    if !remember.on || toplevel.parent().is_some() {
        return None;
    }
    let id = app_id(toplevel)?;
    remember
        .app_ids
        .insert(toplevel.wl_surface().clone(), id.clone());
    let spot = remember.apps.get(&id)?;

    if rules.compute_open_floating(toplevel) {
        if let Some((w, h)) = spot.floating_size {
            rules
                .default_width
                .get_or_insert(Some(PresetSize::Fixed(w.round() as i32)));
            rules
                .default_height
                .get_or_insert(Some(PresetSize::Fixed(h.round() as i32)));
        }
        if let Some((x, y)) = spot.floating_pos {
            rules
                .default_floating_position
                .get_or_insert(FloatingPosition {
                    x: FloatOrInt(x),
                    y: FloatOrInt(y),
                    relative_to: RelativeTo::TopLeft,
                });
        }
    } else if let Some(w) = spot.tiled_width {
        rules
            .default_width
            .get_or_insert(Some(PresetSize::Fixed(w.round() as i32)));
    }

    // the place: only when the rules name none, and only on an output still there
    if rules.open_on_output.is_some() || rules.open_on_workspace.is_some() {
        return None;
    }
    let mut on_output = layout
        .workspaces()
        .filter(|(mon, _, _)| mon.is_some_and(|m| m.output().name() == spot.output))
        .peekable();
    on_output.peek()?;
    rules.open_on_output = Some(spot.output.clone());
    if let Some(name) = &spot.workspace {
        if layout.find_workspace_by_name(name).is_some() {
            rules.open_on_workspace = Some(name.clone());
        }
        return None;
    }
    on_output
        .find(|(_, idx, _)| *idx == spot.workspace_idx)
        .map(|(_, _, ws)| ws.id())
}
