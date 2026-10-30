//! angel-niri: a clean desk on top. `niri msg action set-desk-zero [--on false]`.
//!
//! On, every monitor keeps an empty workspace above its first one (niri's own
//! `empty-workspace-above-first`) and numbers it 0: the desks with windows stay 1…N in IPC,
//! `focus-workspace N`, `move-workspace-to-index`, the overview and the remembered places. A window
//! put on desk 0 makes it desk 1 and a new empty desk 0 comes above. Off, as stock niri. The
//! config's own `empty-workspace-above-first` alone keeps stock numbering.

#[cfg(not(test))]
static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// tests run side by side, each on its own thread: one turning it on mustn't renumber the others
#[cfg(test)]
thread_local! {
    static ON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn set(on: bool) {
    #[cfg(not(test))]
    ON.store(on, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    ON.set(on);
}

pub fn on() -> bool {
    #[cfg(not(test))]
    return ON.load(std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    ON.get()
}
