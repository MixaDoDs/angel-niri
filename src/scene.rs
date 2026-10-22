//! angel-niri: windows behind a layered wallpaper's nearer things.
//!
//! A shell marks a window (`niri msg action set-window-behind-scene --id N`); while it is floating,
//! in its normal size, without the pointer over it and with the overview closed, it is drawn under
//! the bottom layer-shell layer instead of above it: a shell surface on that layer (drawing the
//! scene's nearer layers again, click-through) then stands in front of it. The pointer over it
//! brings it forward whole, and so does a workspace switch on its output (the bottom layer slides
//! with the workspaces while a backdrop wallpaper stays: the nearer layers would slide away from
//! the scene). Input is unchanged: a click-through surface lets clicks reach it.
//! Nothing goes to niri's config, so stock niri simply doesn't know the action.

use crate::layout::LayoutElement as _;
use crate::niri::Niri;

impl Niri {
    pub fn set_window_behind_scene(&mut self, id: u64, behind: bool) {
        self.layout.with_windows_mut(|mapped, _| {
            if mapped.id().get() == id {
                mapped.set_behind_scene(behind);
            }
        });
        self.refresh_behind_scene();
        self.queue_redraw_all();
    }

    /// Works out which marked windows are drawn behind now; redraws when that changed.
    pub fn refresh_behind_scene(&mut self) {
        let hovered = self
            .pointer_contents
            .window
            .as_ref()
            .map(|(w, _)| w.clone());
        let overview = self.layout.is_overview_open();
        let switching: Vec<_> = (self.layout.monitors())
            .filter(|mon| mon.is_switching_workspaces())
            .map(|mon| mon.output().clone())
            .collect();
        let mut changed = false;
        self.layout.with_windows_mut(|mapped, output| {
            let behind = mapped.behind_scene()
                && mapped.is_floating()
                && mapped.sizing_mode().is_normal()
                && !overview
                && !output.is_some_and(|o| switching.contains(o))
                && hovered.as_ref() != Some(&mapped.window);
            changed |= mapped.set_render_behind_scene(behind);
        });
        if changed {
            self.queue_redraw_all();
        }
    }
}
