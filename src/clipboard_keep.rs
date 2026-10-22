//! angel-niri: the clipboard outlives the program it was copied from.
//! `niri msg action set-clipboard-keep [--on false]`.
//!
//! On Wayland the program that copied serves its data itself, so closing it empties the
//! clipboard. Here, when a program sets the clipboard, niri reads its text, links, files and one
//! image (PNG first) into memory, and while the program lives it keeps serving as always. Once it
//! is gone, niri takes the clipboard over with what it read. Nothing goes to disk; password
//! managers' copies (`x-kde-passwordManagerHint`) are never read; past `MAX_BYTES` it gives up.
//! Only the clipboard (Ctrl+C), not the primary selection (middle click).

use std::io::{ErrorKind, Read};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use smithay::reexports::calloop::channel::{self, Sender};
use smithay::reexports::rustix::fs::{fcntl_setfl, OFlags};
use smithay::reexports::rustix::pipe::{pipe_with, PipeFlags};
use smithay::wayland::selection::data_device::{
    current_data_device_selection_userdata, request_data_device_client_selection,
    set_data_device_selection,
};
use smithay::wayland::selection::SelectionSource;

use crate::niri::State;

static ON: AtomicBool = AtomicBool::new(false);

pub fn set(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// More than this in one copy is not kept.
const MAX_BYTES: usize = 64 << 20;
/// A program slower than this to hand its data over is not waited for.
const READ_FOR: Duration = Duration::from_secs(3);
/// At most this many formats of one copy are kept.
const MAX_TYPES: usize = 16;

/// What was read: (copy number, [(mime type, data)]).
type Got = (u64, Vec<(String, Arc<[u8]>)>);

#[derive(Default)]
pub struct ClipboardKeep {
    /// Counts copies, so data read for an older one is dropped.
    copy: u64,
    /// The program's source of the current copy, until niri takes it over.
    source: Option<SelectionSource>,
    /// Its formats read so far.
    data: Vec<(String, Arc<[u8]>)>,
    /// The program is gone; take over once there is data.
    gone: bool,
    /// The user data of niri's own clipboard while it serves the kept copy.
    serving: Option<Arc<[u8]>>,
    tx: Option<Sender<Got>>,
}

/// The formats worth keeping: text of any kind, links and files, one image.
fn wanted(mimes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = mimes
        .iter()
        .filter(|m| {
            m.starts_with("text/")
                || matches!(
                    m.as_str(),
                    "UTF8_STRING"
                        | "STRING"
                        | "TEXT"
                        | "COMPOUND_TEXT"
                        | "x-special/gnome-copied-files"
                        | "application/x-kde-cutselection"
                )
        })
        .cloned()
        .collect();
    let image = mimes
        .iter()
        .find(|m| *m == "image/png")
        .or_else(|| mimes.iter().find(|m| m.starts_with("image/")));
    out.extend(image.cloned());
    out.truncate(MAX_TYPES);
    out
}

/// Reads the pipes one after another, nonblocking, until `READ_FOR` or `MAX_BYTES` runs out.
fn read_all(pipes: Vec<(String, OwnedFd)>) -> Option<Vec<(String, Arc<[u8]>)>> {
    let until = Instant::now() + READ_FOR;
    let mut total = 0;
    let mut out = vec![];
    for (mime, fd) in pipes {
        if let Err(err) = fcntl_setfl(&fd, OFlags::NONBLOCK) {
            warn!("clipboard keep: error setting a pipe nonblocking: {err:?}");
            continue;
        }
        let mut file = std::fs::File::from(fd);
        let mut buf = vec![];
        let mut chunk = [0u8; 64 << 10];
        loop {
            match file.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if total > MAX_BYTES {
                        return None;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() > until {
                        return None;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => (),
                Err(_) => break,
            }
        }
        if !buf.is_empty() {
            out.push((mime, Arc::from(buf)));
        }
    }
    Some(out)
}

impl State {
    /// A program set the clipboard (or cleared it: `None`).
    pub fn clipboard_keep_new(&mut self, source: Option<SelectionSource>) {
        let keep = &mut self.niri.clipboard_keep;
        keep.copy += 1;
        keep.data.clear();
        keep.gone = false;
        keep.serving = None;
        keep.source = None;

        let Some(source) = source else { return };
        let mimes = source.mime_types();
        if !ON.load(Ordering::Relaxed) || mimes.iter().any(|m| m == "x-kde-passwordManagerHint") {
            return;
        }
        let mimes = wanted(&mimes);
        if mimes.is_empty() {
            return;
        }
        keep.source = Some(source);

        // smithay hands the new source to the seat right after this call: read it from there.
        let copy = keep.copy;
        self.niri
            .event_loop
            .insert_idle(move |state| state.clipboard_keep_read(copy, mimes));
    }

    fn clipboard_keep_read(&mut self, copy: u64, mimes: Vec<String>) {
        if self.niri.clipboard_keep.copy != copy {
            return;
        }

        let mut pipes = vec![];
        for mime in mimes {
            let (read, write) = match pipe_with(PipeFlags::CLOEXEC) {
                Ok(fds) => fds,
                Err(err) => {
                    warn!("clipboard keep: error creating a pipe: {err:?}");
                    return;
                }
            };
            match request_data_device_client_selection::<State>(
                &self.niri.seat,
                mime.clone(),
                write,
            ) {
                Ok(()) => pipes.push((mime, read)),
                // Not a program's clipboard any more.
                Err(_) => return,
            }
        }

        let tx = self.niri.clipboard_keep.tx.get_or_insert_with(|| {
            let (tx, rx) = channel::channel::<Got>();
            self.niri
                .event_loop
                .insert_source(rx, |event, _, state| {
                    if let channel::Event::Msg((copy, data)) = event {
                        let keep = &mut state.niri.clipboard_keep;
                        if keep.copy == copy {
                            keep.data = data;
                            state.clipboard_keep_take_over();
                        }
                    }
                })
                .unwrap();
            tx
        });
        let tx = tx.clone();
        thread::spawn(move || {
            if let Some(data) = read_all(pipes) {
                let _ = tx.send((copy, data));
            }
        });
    }

    /// Notices the program that copied going away (called every refresh).
    pub fn clipboard_keep_refresh(&mut self) {
        let keep = &mut self.niri.clipboard_keep;
        if keep.gone || keep.source.as_ref().is_none_or(SelectionSource::alive) {
            return;
        }
        keep.gone = true;
        self.clipboard_keep_take_over();
    }

    fn clipboard_keep_take_over(&mut self) {
        let keep = &mut self.niri.clipboard_keep;
        if !keep.gone || keep.data.is_empty() || keep.source.is_none() {
            return;
        }
        keep.source = None;
        // niri itself has set the clipboard since (a screenshot): leave it.
        if current_data_device_selection_userdata::<State>(&self.niri.seat).is_some() {
            return;
        }

        let mimes = keep.data.iter().map(|(m, _)| m.clone()).collect();
        let serving: Arc<[u8]> = Arc::from([]);
        keep.serving = Some(serving.clone());
        debug!("clipboard keep: the program is gone, serving its copy");
        set_data_device_selection(&self.niri.display_handle, &self.niri.seat, mimes, serving);
    }
}

impl ClipboardKeep {
    /// The kept data of `mime` when niri's clipboard with `user_data` is the kept copy.
    pub fn data_for(&self, mime: &str, user_data: &Arc<[u8]>) -> Option<Arc<[u8]>> {
        let serving = self.serving.as_ref()?;
        if !Arc::ptr_eq(serving, user_data) {
            return None;
        }
        self.data
            .iter()
            .find(|(m, _)| m == mime)
            .map(|(_, data)| data.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wanted_keeps_text_and_one_image() {
        let mimes: Vec<String> = [
            "text/plain;charset=utf-8",
            "text/html",
            "image/bmp",
            "image/png",
            "application/x-openoffice-embed-source-xml",
            "UTF8_STRING",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            wanted(&mimes),
            [
                "text/plain;charset=utf-8",
                "text/html",
                "UTF8_STRING",
                "image/png"
            ]
        );
    }
}
