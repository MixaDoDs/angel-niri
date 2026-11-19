//! One toplevel capture session used by the AngelOS desktop.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::os::fd::{AsFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context as _, Result};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use memmap2::{Mmap, MmapOptions};
use niri::window::mapped::MappedId;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{
    delegate_noop, event_created_child, Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1 as handle, ext_foreign_toplevel_list_v1 as list,
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1 as frame, ext_image_copy_capture_manager_v1 as manager,
    ext_image_copy_capture_session_v1 as session,
};

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_SHM_BYTES: usize = 128 * 1024 * 1024;

pub fn run(id: u64, directory: &Path, once: bool, crop: &str, limit: (u32, u32)) -> Result<()> {
    let crop = Crop::parse(crop)?;
    let directory = private_directory(directory)?;
    let connection = Connection::connect_to_env().context("connect to Wayland")?;
    let (globals, mut queue) = registry_queue_init::<CaptureState>(&connection)?;
    let qh = queue.handle();
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let source_manager: ExtForeignToplevelImageCaptureSourceManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .context("Wayland window capture is unavailable")?;
    let copy_manager: manager::ExtImageCopyCaptureManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .context("Wayland image copy capture is unavailable")?;
    let toplevels: list::ExtForeignToplevelListV1 = globals
        .bind(&qh, 1..=1, ())
        .context("Wayland toplevel list is unavailable")?;

    let mut state = CaptureState {
        identifier: Some(MappedId::specific(id).to_protocol_identifier()),
        ..CaptureState::default()
    };
    queue.roundtrip(&mut state)?;
    let toplevel = state
        .toplevel
        .take()
        .with_context(|| format!("window {id} was not found"))?;
    state.identifier = None;
    let source = source_manager.create_source(&toplevel, &qh, ());
    let session = copy_manager.create_session(&source, manager::Options::empty(), &qh, ());
    source.destroy();
    source_manager.destroy();
    copy_manager.destroy();
    toplevel.destroy();
    toplevels.stop();

    let mut event_loop = EventLoop::try_new()?;
    WaylandSource::new(connection, queue)
        .insert(event_loop.handle())
        .map_err(|err| err.error)?;

    let mut next_capture = Instant::now();
    let first_deadline = next_capture + FIRST_FRAME_TIMEOUT;
    let mut first = true;
    let mut slot = 0;
    let mut buffer: Option<SharedBuffer> = None;
    let mut stdout = io::stdout().lock();

    loop {
        ensure!(!state.stopped, "window capture stopped");
        if let Some(result) = state.result.take() {
            match result {
                FrameResult::Ready(Some(WEnum::Value(wl_output::Transform::Normal))) => {
                    let basename = if slot == 0 {
                        "frame-0.ppm"
                    } else {
                        "frame-1.ppm"
                    };
                    buffer
                        .as_ref()
                        .unwrap()
                        .save(&directory, basename, &crop, limit)?;
                    writeln!(stdout, "{basename}")?;
                    stdout.flush()?;
                    if once {
                        return Ok(());
                    }
                    first = false;
                    slot ^= 1;
                }
                FrameResult::Ready(transform) => {
                    bail!("unsupported window capture transform: {transform:?}")
                }
                FrameResult::Failed(WEnum::Value(frame::FailureReason::BufferConstraints)) => {
                    // A resize may invalidate a frame after we requested it. The next
                    // constraints batch is processed before allocating another buffer.
                    buffer.take();
                }
                FrameResult::Failed(reason) => bail!("window capture failed: {reason:?}"),
            }
        }

        let now = Instant::now();
        ensure!(
            !first || now < first_deadline,
            "timed out waiting for a window frame"
        );
        let mut wait = first.then(|| first_deadline.saturating_duration_since(now));

        if !state.in_flight {
            if let Some(constraints) = state.constraints {
                if now < next_capture {
                    let delay = next_capture - now;
                    wait = Some(wait.map_or(delay, |wait| wait.min(delay)));
                } else {
                    let params = constraints.parameters()?;
                    let fresh = buffer.as_ref().is_none_or(|buffer| buffer.params != params);
                    if fresh {
                        buffer.take();
                        buffer = Some(SharedBuffer::new(&shm, &qh, params)?);
                    }
                    let frame = session.create_frame(&qh, ());
                    frame.attach_buffer(&buffer.as_ref().unwrap().proxy);
                    if fresh {
                        frame.damage_buffer(0, 0, params.width, params.height);
                    }
                    frame.capture();
                    state.transform = None;
                    state.in_flight = true;
                    next_capture = Instant::now() + FRAME_INTERVAL;
                }
            }
        }

        event_loop.dispatch(wait, &mut state)?;
    }
}

fn private_directory(directory: &Path) -> Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is missing")?;
    let runtime = Path::new(&runtime).canonicalize()?;
    let directory = directory.canonicalize()?;
    let metadata = directory.metadata()?;
    ensure!(
        directory != runtime
            && directory.starts_with(&runtime)
            && metadata.is_dir()
            && metadata.permissions().mode() & 0o077 == 0
            && metadata.uid() == unsafe { libc::geteuid() },
        "capture directory must be a private directory under XDG_RUNTIME_DIR"
    );
    Ok(directory)
}

#[derive(Default, Clone, Copy)]
struct Constraints {
    size: Option<(u32, u32)>,
    format: Option<wl_shm::Format>,
}

impl Constraints {
    fn parameters(self) -> Result<BufferParameters> {
        let (width, height) = self.size.context("missing window capture dimensions")?;
        let format = self
            .format
            .context("no supported window capture SHM format")?;
        let stride = width
            .checked_mul(4)
            .context("window capture row is too wide")?;
        let bytes = stride as u64 * height as u64;
        ensure!(
            width > 0 && height > 0 && bytes <= MAX_SHM_BYTES as u64,
            "invalid window capture size {width}×{height}"
        );
        Ok(BufferParameters {
            width: width as i32,
            height: height as i32,
            stride: stride as i32,
            bytes: bytes as usize,
            format,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BufferParameters {
    width: i32,
    height: i32,
    stride: i32,
    bytes: usize,
    format: wl_shm::Format,
}

struct SharedBuffer {
    params: BufferParameters,
    proxy: wl_buffer::WlBuffer,
    pixels: Mmap,
}

impl SharedBuffer {
    fn new(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<CaptureState>,
        params: BufferParameters,
    ) -> Result<Self> {
        let fd = unsafe {
            libc::memfd_create(
                c"niri-window-crop".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error()).context("allocate a window capture buffer");
        }

        // memfd_create returned a new descriptor owned by this process.
        let file = unsafe { File::from_raw_fd(fd) };
        file.set_len(params.bytes as u64)?;
        let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
        if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 {
            return Err(io::Error::last_os_error()).context("seal the window capture buffer size");
        }
        // The file cannot change size. The compositor can write only during a capture;
        // we read pixels after the corresponding ready event and before the next request.
        let pixels = unsafe { MmapOptions::new().map(&file)? };
        let pool = shm.create_pool(file.as_fd(), params.bytes as i32, qh, ());
        let proxy = pool.create_buffer(
            0,
            params.width,
            params.height,
            params.stride,
            params.format,
            qh,
            (),
        );
        pool.destroy();
        Ok(Self {
            params,
            proxy,
            pixels,
        })
    }

    fn save(&self, directory: &Path, basename: &str, crop: &Crop, limit: (u32, u32)) -> Result<()> {
        let temporary = directory.join(".frame.tmp");
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        let mut writer = BufWriter::new(file);
        crop.write_ppm(
            &mut writer,
            &self.pixels,
            self.params.width as usize,
            self.params.height as usize,
            limit,
        )?;
        writer.flush()?;
        fs::rename(temporary, directory.join(basename))?;
        Ok(())
    }
}

impl Drop for SharedBuffer {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}

struct Crop {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Crop {
    fn parse(value: &str) -> Result<Self> {
        let mut numbers = value.split(',').map(str::parse::<f64>);
        let mut next = || {
            numbers
                .next()
                .context("expected X,Y,W,H")?
                .context("expected a number in X,Y,W,H")
        };
        let (x, y, width, height) = (next()?, next()?, next()?, next()?);
        ensure!(
            numbers.next().is_none()
                && (0.0..1.0).contains(&x)
                && (0.0..1.0).contains(&y)
                && width > 0.0
                && height > 0.0
                && x + width <= 1.0 + f64::EPSILON
                && y + height <= 1.0 + f64::EPSILON,
            "crop must be a non-empty rectangle inside the unit square"
        );
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    fn write_ppm(
        &self,
        writer: &mut impl Write,
        pixels: &[u8],
        width: usize,
        height: usize,
        limit: (u32, u32),
    ) -> io::Result<()> {
        let axis = |start: f64, span: f64, dimension: usize| {
            let start_px = ((start * dimension as f64).round() as usize).min(dimension - 1);
            let end_px = (((start + span) * dimension as f64).round() as usize)
                .clamp(start_px + 1, dimension);
            (start_px, end_px - start_px)
        };
        let (x, crop_width) = axis(self.x, self.width, width);
        let (y, crop_height) = axis(self.y, self.height, height);
        let scale = (limit.0 as f64 / crop_width as f64)
            .min(limit.1 as f64 / crop_height as f64)
            .min(1.0);
        let output_width = ((crop_width as f64 * scale).round() as usize).max(1);
        let output_height = ((crop_height as f64 * scale).round() as usize).max(1);
        write!(writer, "P6\n{output_width} {output_height}\n255\n")?;

        let mut row = vec![0; output_width * 3];
        for dest_y in 0..output_height {
            let src_y = y + (2 * dest_y + 1) * crop_height / (2 * output_height);
            for (dest_x, rgb) in row.chunks_exact_mut(3).enumerate() {
                let src_x = x + (2 * dest_x + 1) * crop_width / (2 * output_width);
                let offset = (src_y * width + src_x) * 4;
                // wl_shm XRGB8888 and ARGB8888 both store little-endian B, G, R, X/A.
                rgb.copy_from_slice(&[pixels[offset + 2], pixels[offset + 1], pixels[offset]]);
            }
            writer.write_all(&row)?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct CaptureState {
    identifier: Option<String>,
    toplevel: Option<handle::ExtForeignToplevelHandleV1>,
    pending_constraints: Constraints,
    constraints: Option<Constraints>,
    stopped: bool,
    in_flight: bool,
    transform: Option<WEnum<wl_output::Transform>>,
    result: Option<FrameResult>,
}

enum FrameResult {
    Ready(Option<WEnum<wl_output::Transform>>),
    Failed(WEnum<frame::FailureReason>),
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for CaptureState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<list::ExtForeignToplevelListV1, ()> for CaptureState {
    fn event(
        _: &mut Self,
        list: &list::ExtForeignToplevelListV1,
        event: list::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let list::Event::Finished = event {
            list.destroy();
        }
    }

    event_created_child!(Self, list::ExtForeignToplevelListV1, [
        list::EVT_TOPLEVEL_OPCODE => (handle::ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<handle::ExtForeignToplevelHandleV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        toplevel: &handle::ExtForeignToplevelHandleV1,
        event: handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            handle::Event::Identifier { identifier }
                if state.identifier.as_ref() == Some(&identifier) =>
            {
                state.toplevel = Some(toplevel.clone());
            }
            handle::Event::Done if state.toplevel.as_ref() != Some(toplevel) => {
                toplevel.destroy();
            }
            handle::Event::Closed => {
                if state.toplevel.as_ref() == Some(toplevel) {
                    state.toplevel = None;
                }
                toplevel.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<session::ExtImageCopyCaptureSessionV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _: &session::ExtImageCopyCaptureSessionV1,
        event: session::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            session::Event::BufferSize { width, height } => {
                state.pending_constraints.size = Some((width, height));
            }
            session::Event::ShmFormat {
                format: WEnum::Value(wl_shm::Format::Xrgb8888),
            } => {
                state.pending_constraints.format = Some(wl_shm::Format::Xrgb8888);
            }
            session::Event::ShmFormat {
                format: WEnum::Value(wl_shm::Format::Argb8888),
            } if state.pending_constraints.format.is_none() => {
                state.pending_constraints.format = Some(wl_shm::Format::Argb8888);
            }
            session::Event::Done => {
                state.constraints = Some(std::mem::take(&mut state.pending_constraints));
            }
            session::Event::Stopped => state.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<frame::ExtImageCopyCaptureFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        frame: &frame::ExtImageCopyCaptureFrameV1,
        event: frame::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let result = match event {
            frame::Event::Transform { transform } => {
                state.transform = Some(transform);
                return;
            }
            frame::Event::Ready => FrameResult::Ready(state.transform),
            frame::Event::Failed { reason } => FrameResult::Failed(reason),
            _ => return,
        };
        frame.destroy();
        state.in_flight = false;
        state.result = Some(result);
    }
}

delegate_noop!(CaptureState: ignore wl_shm::WlShm);
delegate_noop!(CaptureState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(CaptureState: ignore wl_buffer::WlBuffer);
delegate_noop!(CaptureState: ignore ExtForeignToplevelImageCaptureSourceManagerV1);
delegate_noop!(CaptureState: ignore ExtImageCaptureSourceV1);
delegate_noop!(CaptureState: ignore manager::ExtImageCopyCaptureManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_crop_keeps_rgb_order_and_downsamples() {
        let pixels = [
            1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0, 10, 11, 12, 0, 13, 14, 15, 0, 16, 17, 18, 0, 19,
            20, 21, 0, 22, 23, 24, 0,
        ];
        let mut ppm = Vec::new();
        Crop::parse("0.5,0,0.5,1")
            .unwrap()
            .write_ppm(&mut ppm, &pixels, 4, 2, (1, 1))
            .unwrap();
        assert_eq!(ppm, b"P6\n1 1\n255\n\x18\x17\x16");
    }

    #[test]
    fn normalized_crop_rejects_empty_and_out_of_bounds_rectangles() {
        for value in [
            "0,0,0,1",
            "-1,0,1,1",
            "0,0,NaN,1",
            "0.9,0,0.2,1",
            "0,0,1,1,1",
        ] {
            assert!(Crop::parse(value).is_err(), "{value}");
        }
    }
}
