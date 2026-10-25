//! angel-niri: a program opens where you told it to. `niri msg action set-app-place --app-id ID
//! --output O --workspace N [--floating true]` pins a program's place (without `--output` it is
//! forgotten); `set-remember-windows [--on false]` turns the pins on or off without losing them.
//! The pins live in `$XDG_STATE_HOME/angel-niri/windows.json` with the switch, so they work from
//! niri's start, before a shell is up; nothing goes to niri's config.
//!
//! A pinned program's first window opens on that output and workspace (by name, else its index on
//! the output), floating or not, and the view goes there with it. Nothing else is remembered: not
//! the size, not the position; a second window of the program, dialogs and a missing output open
//! as always. Window rules from the config always win: only what they leave unset is filled in.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::layout::workspace::WorkspaceId;
use crate::layout::Layout;
use crate::niri::Niri;
use crate::utils::with_toplevel_role;
use crate::window::{Mapped, ResolvedWindowRules};

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Spot {
    pub output: String,
    #[serde(default)]
    pub workspace: Option<String>,
    /// Its index on the output, from 0.
    #[serde(default)]
    pub workspace_idx: usize,
    #[serde(default)]
    pub floating: bool,
}

// (the automatic places of before, under "apps", are dropped: the owner wiped them, 23.10)
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Remember {
    pub on: bool,
    #[serde(default)]
    pub pins: HashMap<String, Spot>,
    /// Workspaces picked by index for windows not mapped yet.
    #[serde(skip)]
    pub targets: HashMap<WlSurface, WorkspaceId>,
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

impl Remember {
    /// Whether the window's program has a pinned place (Desktop leaves it where it is).
    pub fn is_pinned(&self, mapped: &Mapped) -> bool {
        self.on && app_id(mapped.toplevel()).is_some_and(|id| self.pins.contains_key(&id))
    }
}

fn app_id(toplevel: &ToplevelSurface) -> Option<String> {
    with_toplevel_role(toplevel, |role| role.app_id.clone()).filter(|id| !id.is_empty())
}

impl Niri {
    pub fn set_remember_windows(&mut self, on: bool) {
        self.remember.on = on;
        self.remember.save();
    }

    /// Pin a program's place, or forget it (`None`).
    pub fn set_app_place(&mut self, app_id: String, spot: Option<Spot>) {
        match spot {
            Some(spot) => self.remember.pins.insert(app_id, spot),
            None => self.remember.pins.remove(&app_id),
        };
        self.remember.save();
    }
}

/// At a new window's initial configure: what the rules leave unset, from its program's pin. A
/// workspace picked by index is returned, to be used when the window maps.
pub fn apply(
    remember: &Remember,
    layout: &Layout<Mapped>,
    rules: &mut ResolvedWindowRules,
    toplevel: &ToplevelSurface,
    is_at_startup: bool,
) -> Option<WorkspaceId> {
    if !remember.on || toplevel.parent().is_some() {
        return None;
    }
    let id = app_id(toplevel)?;
    let spot = remember.pins.get(&id)?;
    // only the program's first window: another one open (or minimized) already
    let same = |m: &Mapped| app_id(m.toplevel()).as_deref() == Some(id.as_str());
    if layout.windows().any(|(_, m)| same(m)) || layout.minimized_windows().any(same) {
        return None;
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
    rules.open_floating.get_or_insert(spot.floating);
    // the view goes with it, but not for the programs started with the session
    if !is_at_startup {
        rules.open_focused.get_or_insert(true);
    }
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
