//! Images for frontends to draw: decoded once, off the editor's thread,
//! and shown by any mode as embedded blocks (EDITOR.md, sections 2 and 3:
//! a projection, `(block AT IMAGE)`). Decoding is Rust's (the `image`
//! crate's raster formats, `resvg` for SVG): a format parser in Scheme
//! would be far too slow for a picture, and the pixels go to the GPU.
//!
//! An image is its pixels, RGBA in sRGB with straight (not premultiplied)
//! alpha, with an identity frontends cache textures by: a frontend
//! premultiplies where it blends, in linear light. Pictures larger than
//! `MAX_SIDE` on a side are scaled down as they are decoded: a frontend
//! draws them smaller than that anyway. Lisp loads images by path
//! (`image-load`, which the calling task waits for while one of a few
//! decoding threads works), and a loaded image is kept by path and
//! modification time, so a view rendered again does not decode again.
//!
//! What a picture may cost is bounded before it is decoded: its file at
//! most `MAX_FILE` bytes, its dimensions at most `MAX_DECODED` a side and
//! `MAX_PIXELS` in all, and no more than `DECODERS` decoding at once. An
//! SVG reads nothing but itself: references to other files are not
//! followed, whatever the world may read (they would be read unchecked).

use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

use techne_vm::{
    api::{Foreign, Value},
    vm::Vm,
};

/// The longest side an image keeps, in pixels.
pub const MAX_SIDE: u32 = 4096;

/// The largest image file read, in bytes.
pub const MAX_FILE: u64 = 64 << 20;

/// The largest dimensions decoded, a side and in all, before scaling.
pub const MAX_DECODED: u32 = 16384;
pub const MAX_PIXELS: u64 = 64 << 20;

/// How many images decode at once.
const DECODERS: usize = 2;

/// How many bytes of decoded images the cache keeps, past which the
/// least recently loaded go (those still shown stay alive where shown).
const CACHE_BYTES: usize = 256 << 20;

/// An image's pixels.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ImageData {
    /// Never reused within the process: frontends key textures by it.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// RGBA, row by row, in sRGB, alpha straight.
    #[serde(with = "serde_bytes")]
    pub rgba: Vec<u8>,
}

/// A decoded image, shared: cheap to clone into snapshots.
#[derive(Clone)]
pub struct Image(pub Arc<ImageData>);

/// An image is sent by its id and size: a frontend in another process is
/// sent its pixels once (`attach`), and puts them back by its id.
impl serde::Serialize for Image {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.0.id, self.0.width, self.0.height).serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for Image {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Image, D::Error> {
        let (id, width, height) = <(u64, u32, u32)>::deserialize(d)?;
        Ok(Image(Arc::new(ImageData { id, width, height, rgba: Vec::new() })))
    }
}

impl Image {
    pub fn id(&self) -> u64 {
        self.0.id
    }
    pub fn size(&self) -> (u32, u32) {
        (self.0.width, self.0.height)
    }
}

impl PartialEq for Image {
    fn eq(&self, other: &Image) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for Image {}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Image#{}({}x{})", self.0.id, self.0.width, self.0.height)
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);

fn image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    Image(Arc::new(ImageData { id: NEXT.fetch_add(1, Ordering::Relaxed), width, height, rgba }))
}

/// Encode GPU readback pixels without moving file or capture policy out
/// of Scheme. The existing image codec also handles decoding here. A VM
/// prototype took 27.6 s for just CRC/Adler passes over a 1600x1000 frame;
/// native bulk encoding is needed even for low-rate recording.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    if width == 0 || height == 0 || width as u64 * height as u64 > 16_777_216 || rgba.len() as u64 != width as u64 * height as u64 * 4 {
        return Err("capture dimensions exceed the 16 megapixel limit or do not match the pixels".into());
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(png)
}

/// Whether `bytes` (of a file named `name`) are SVG.
fn is_svg(name: &Path, bytes: &[u8]) -> bool {
    let ext = name.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("svg" | "svgz")) {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let head = head.trim_start();
    head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg"))
}

/// The system's fonts, for text in SVG: loaded once, the first time.
fn fonts() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            db.load_system_fonts();
            // The generic families as the fonts installed have them, not
            // fontdb's defaults (Arial, Times New Roman), which a system
            // may lack: then an SVG's text would not show at all.
            let has = |name: &str| db.faces().any(|f| f.families.iter().any(|(n, _)| n == name));
            let any = db.faces().next().and_then(|f| f.families.first().map(|(n, _)| n.clone()));
            let pick = |names: &[&str]| names.iter().find(|n| has(n)).map(|n| n.to_string()).or_else(|| any.clone());
            let sans = pick(&["Open Sans", "Noto Sans", "DejaVu Sans", "Liberation Sans", "Cantarell", "Inter", "Arial"]);
            let serif = pick(&["Noto Serif", "DejaVu Serif", "Liberation Serif", "Times New Roman"]);
            let mono = pick(&["Noto Sans Mono", "DejaVu Sans Mono", "Liberation Mono", "JetBrains Mono"]);
            if let Some(f) = sans {
                db.set_sans_serif_family(f);
            }
            if let Some(f) = serif {
                db.set_serif_family(f);
            }
            if let Some(f) = mono {
                db.set_monospace_family(f);
            }
            Arc::new(db)
        })
        .clone()
}

/// The size `(w, h)` scaled to fit `MAX_SIDE`, at least a pixel a side.
fn fitted(w: f32, h: f32) -> (u32, u32, f32) {
    let scale = (MAX_SIDE as f32 / w.max(h)).min(1.0);
    (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1), scale)
}

/// Whether `bytes` are gzip's: a compressed SVG, which would be inflated
/// without a bound before anything could be checked.
fn gzipped(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

/// Whether the raster in `bytes`, if they are one, is within what may be
/// decoded: what the image crate cannot read the dimensions of is left to
/// the caller (an SVG nested in an SVG).
fn raster_allowed(bytes: &[u8]) -> Result<(), String> {
    let Ok(reader) = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format() else { return Ok(()) };
    if reader.format().is_none() {
        return Ok(());
    }
    let (w, h) = reader.into_dimensions().map_err(|e| e.to_string())?;
    if w > MAX_DECODED || h > MAX_DECODED || w as u64 * h as u64 > MAX_PIXELS {
        return Err(format!("{w}x{h} is too large to show"));
    }
    Ok(())
}

/// Decode the image in `bytes`, from a file named `name`.
pub fn decode(name: &Path, bytes: &[u8]) -> Result<Image, String> {
    if gzipped(bytes) {
        return Err("a compressed SVG is not shown".into());
    }
    if is_svg(name, bytes) {
        // No resources directory, and hrefs that are not data: URLs
        // resolve to nothing: an SVG reads no other file. A data: URL's
        // picture is held to what a picture may cost, as a file's is.
        let data = resvg::usvg::ImageHrefResolver::default_data_resolver();
        let resolver = resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(move |mime: &str, bytes: Arc<Vec<u8>>, options: &resvg::usvg::Options| {
                if gzipped(&bytes) || raster_allowed(&bytes).is_err() {
                    return None;
                }
                data(mime, bytes, options)
            }),
            resolve_string: Box::new(|_, _| None),
        };
        let options = resvg::usvg::Options { fontdb: fonts(), resources_dir: None, image_href_resolver: resolver, ..Default::default() };
        let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
        let size = tree.size();
        let (w, h, scale) = fitted(size.width(), size.height());
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("an empty SVG")?;
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        // tiny-skia's pixels are premultiplied: straightened.
        let mut pixels = pixmap.take();
        for p in pixels.as_chunks_mut::<4>().0 {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        return Ok(image(w, h, pixels));
    }
    raster_allowed(bytes)?;
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut decoded = reader.decode().map_err(|e| e.to_string())?;
    let (w, h, _) = fitted(decoded.width() as f32, decoded.height() as f32);
    if (w, h) != (decoded.width(), decoded.height()) {
        decoded = decoded.resize(w, h, image::imageops::FilterType::Triangle);
    }
    let rgba = decoded.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(image(w, h, rgba.into_raw()))
}

/// Decode the image file at `path`.
pub fn load(path: &Path) -> Result<Image, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    // A regular file only: a FIFO's reader would wait, a decoder with it.
    // Opened without waiting and checked as opened, so a FIFO put in the
    // path's place after a check cannot hold a decoder.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    match file.metadata() {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(format!("{}: not a file", path.display())),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(format!("{}: larger than {} MiB", path.display(), MAX_FILE >> 20));
    }
    decode(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Images loaded, by path and the file's state when they were.
#[derive(Default)]
pub struct Cache {
    entries: HashMap<PathBuf, (Option<SystemTime>, u64, Image, u64)>,
    bytes: usize,
    clock: u64,
}

/// The file's modification time and length, what an entry is good for.
fn stamp(path: &Path) -> Option<(Option<SystemTime>, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok(), m.len()))
}

impl Cache {
    /// The image of `path` if it is loaded and the file has not changed.
    pub fn get(&mut self, path: &Path) -> Option<Image> {
        let now = stamp(path)?;
        self.clock += 1;
        let clock = self.clock;
        let e = self.entries.get_mut(path).filter(|e| (e.0, e.1) == now)?;
        e.3 = clock;
        Some(e.2.clone())
    }

    fn insert(&mut self, path: PathBuf, stamp: (Option<SystemTime>, u64), image: Image) {
        self.clock += 1;
        self.bytes += image.0.rgba.len();
        if let Some(old) = self.entries.insert(path, (stamp.0, stamp.1, image, self.clock)) {
            self.bytes -= old.2.0.rgba.len();
        }
        while self.bytes > CACHE_BYTES && self.entries.len() > 1 {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.3).map(|(p, _)| p.clone()) else { break };
            if let Some(e) = self.entries.remove(&oldest) {
                self.bytes -= e.2.0.rgba.len();
            }
        }
    }
}

/// What a thread of its own computes, as a future the calling task waits on.
pub(crate) struct OnThread<T> {
    shared: Arc<Mutex<(Option<T>, Option<std::task::Waker>)>>,
}

type Job = Box<dyn FnOnce() + Send>;

impl<T: Send + 'static> OnThread<T> {
    /// `work`, and where its result goes.
    fn job(work: impl FnOnce() -> T + Send + 'static) -> (OnThread<T>, Job) {
        let shared = Arc::new(Mutex::new((None, None::<std::task::Waker>)));
        let theirs = shared.clone();
        let job = Box::new(move || {
            let result = work();
            let mut slot = theirs.lock().expect("the result");
            slot.0 = Some(result);
            if let Some(w) = slot.1.take() {
                w.wake();
            }
        });
        (OnThread { shared }, job)
    }

    /// `work` on a thread of its own; on this one if none can start.
    pub(crate) fn spawn(name: &str, work: impl FnOnce() -> T + Send + 'static) -> OnThread<T> {
        let (future, job) = Self::job(work);
        let job = Arc::new(Mutex::new(Some(job)));
        let theirs = job.clone();
        let started = std::thread::Builder::new().name(name.into()).spawn(move || {
            if let Some(job) = theirs.lock().expect("the job").take() {
                job();
            }
        });
        if started.is_err()
            && let Some(job) = job.lock().expect("the job").take()
        {
            job();
        }
        future
    }

    /// `work` on one of the `DECODERS` threads decoding images, after
    /// the work queued before it.
    fn decoding(work: impl FnOnce() -> T + Send + 'static) -> OnThread<T> {
        static QUEUE: OnceLock<Mutex<std::sync::mpsc::Sender<Job>>> = OnceLock::new();
        let (future, job) = Self::job(work);
        let queue = QUEUE.get_or_init(|| {
            let (send, receive) = std::sync::mpsc::channel::<Job>();
            let receive = Arc::new(Mutex::new(receive));
            for i in 0..DECODERS {
                let receive = receive.clone();
                let _ = std::thread::Builder::new().name(format!("image-decode-{i}")).spawn(move || {
                    loop {
                        let job = receive.lock().expect("the queue").recv();
                        match job {
                            Ok(job) => job(),
                            Err(_) => break,
                        }
                    }
                });
            }
            Mutex::new(send)
        });
        // No decoder to take it (none started): decoded here.
        if let Err(std::sync::mpsc::SendError(job)) = queue.lock().expect("the queue").send(job) {
            job();
        }
        future
    }
}

impl<T> std::future::Future for OnThread<T> {
    type Output = T;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<T> {
        let mut slot = self.shared.lock().expect("the result");
        match slot.0.take() {
            Some(t) => std::task::Poll::Ready(t),
            None => {
                slot.1 = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

type Img = Foreign<Image>;

/// Define the image natives in `vm`.
pub fn install(vm: &mut Vm) {
    let cache: Rc<RefCell<Cache>> = Rc::default();
    let (loaded, loading) = (cache.clone(), cache);
    techne_vm::procedures! { vm;
        /// Return #t if OBJ is an image.
        #[vm]
        "(image? obj)" => |vm: &mut Vm, v: Value| vm.get::<Img>(&v).is_ok();
        /// Return the width of IMAGE in pixels, as decoded.
        "(image-width image)" => |i: Img| i.0.0.width as usize;
        /// Return the height of IMAGE in pixels, as decoded.
        "(image-height image)" => |i: Img| i.0.0.height as usize;
    }
    vm.requiring(techne_vm::vm::Capability::Files, |vm| {
        techne_vm::procedures! { vm;
            /// Return the image of the file at PATH if it is loaded and
            /// the file has not changed since, else #f. It never decodes:
            /// `image-load` does.
            "(image-loaded path)" => move |path: String| -> Option<Img> {
                loaded.borrow_mut().get(Path::new(&path)).map(|i| Foreign(Rc::new(i)))
            };
        }
        // Decoded on a thread of its own, the calling task waiting: a
        // large picture takes longer than a key may.
        vm.register_async("image-load", 1, move |vm: &mut Vm, args: techne_vm::api::Args<'_>| {
            let path: Result<String, String> = args.get(vm, 0).map_err(|e| e.msg.clone());
            let readable = path.as_ref().map(|p| vm.may_read(Path::new(p))).unwrap_or(true);
            let cache = loading.clone();
            async move {
                let path = PathBuf::from(path?);
                if !readable {
                    return Err(format!("{}: outside what this world may read", path.display()));
                }
                if let Some(i) = cache.borrow_mut().get(&path) {
                    return Ok(Foreign(Rc::new(i)));
                }
                let before = stamp(&path).ok_or_else(|| format!("{}: no such file", path.display()))?;
                let theirs = path.clone();
                // A decoder that panics (on a file crafted to make it) is an
                // error: else its thread would end without an answer, and
                // the task waiting for it wait for good.
                let image = OnThread::decoding(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load(&theirs)))
                        .unwrap_or_else(|_| Err(format!("{}: the decoder failed", theirs.display())))
                })
                .await?;
                cache.borrow_mut().insert(path, before, image.clone());
                Ok::<Img, String>(Foreign(Rc::new(image)))
            }
        });
        techne_vm::document! { vm;
            /// Return the image of the file at PATH, decoded on a thread of
            /// its own while the calling task waits: PNG, JPEG, GIF, WebP,
            /// BMP or SVG, scaled down to 4096 pixels a side at most. A
            /// file of more than 64 MiB, or of more than 64 megapixels, is
            /// refused; an SVG's references to other files are not
            /// followed. A file loaded before and not changed since is not
            /// decoded again. Show it with a projection, `(block AT IMAGE)`.
            "(image-load path)";
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasters_and_svg_decode_straight() {
        // A 2x1 PNG: opaque red, then half-transparent white.
        let mut png = Vec::new();
        let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 255, 255, 255, 128]).unwrap();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let i = decode(Path::new("x.png"), &png).unwrap();
        assert_eq!(i.size(), (2, 1));
        assert_eq!(i.0.rgba, [255, 0, 0, 255, 255, 255, 255, 128]);
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4"><rect width="8" height="4" fill="#0000ff"/></svg>"##;
        let s = decode(Path::new("x.svg"), svg).unwrap();
        assert_eq!(s.size(), (8, 4));
        assert_eq!(&s.0.rgba[..4], [0, 0, 255, 255]);
        assert_ne!(i, s, "each its own identity");
        assert!(decode(Path::new("x.png"), b"not an image").is_err());
    }

    #[test]
    fn large_pictures_are_scaled_to_fit() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10000" height="100"></svg>"#;
        assert_eq!(decode(Path::new("x.svg"), svg).unwrap().size(), (MAX_SIDE, 41));
    }

    #[test]
    fn an_svg_reads_no_other_file() {
        // A picture beside it, referred to by path: not drawn.
        let dir = tempfile::tempdir().unwrap();
        let red = br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="#ff0000"/></svg>"##;
        std::fs::write(dir.path().join("secret.svg"), red).unwrap();
        let outer = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image width="4" height="4" href="{0}" xlink:href="{0}"/></svg>"#,
            dir.path().join("secret.svg").display()
        );
        std::fs::write(dir.path().join("outer.svg"), &outer).unwrap();
        let i = load(&dir.path().join("outer.svg")).unwrap();
        assert!(i.0.rgba.iter().all(|&b| b == 0), "nothing of the other file");
        let relative = outer.replace(&dir.path().join("secret.svg").display().to_string(), "secret.svg");
        std::fs::write(dir.path().join("outer.svg"), relative).unwrap();
        assert!(load(&dir.path().join("outer.svg")).unwrap().0.rgba.iter().all(|&b| b == 0));
    }

    #[test]
    fn what_a_picture_may_cost_is_bounded_before_decoding() {
        // A PNG header claiming 100000x100000 pixels: refused unread.
        let mut png = Vec::new();
        image::RgbaImage::new(1, 1).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        png[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        let crc = png[12..29]
            .iter()
            .fold(!0u32, |c, &b| (0..8).fold(c ^ b as u32, |c, _| if c & 1 == 1 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 }));
        png[29..33].copy_from_slice(&(!crc).to_be_bytes());
        let e = decode(Path::new("x.png"), &png).unwrap_err();
        assert!(e.contains("too large"), "{e}");
    }

    #[test]
    fn an_svg_holds_its_pictures_to_what_a_picture_may_cost() {
        // A 20000x1 PNG, small compressed: refused alone, and nested in an
        // SVG as a data: URL, not drawn either.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(20000, 1, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(decode(Path::new("x.png"), &png).unwrap_err().contains("too large"));
        let b64 = |b: &[u8]| {
            const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            b.chunks(3)
                .flat_map(|c| {
                    let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
                    (0..4).map(move |i| if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' })
                })
                .collect::<String>()
        };
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><image width="4" height="4" href="data:image/png;base64,{}"/></svg>"#,
            b64(&png)
        );
        let i = decode(Path::new("x.svg"), svg.as_bytes()).unwrap();
        assert!(i.0.rgba.iter().all(|&b| b == 0), "the picture inside is not drawn");
        // A compressed SVG is refused before it is inflated.
        assert!(decode(Path::new("x.svgz"), &[0x1f, 0x8b, 8, 0]).is_err());
    }

    #[test]
    fn only_a_regular_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo.png");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        assert!(load(&fifo).unwrap_err().contains("not a file"), "refused without waiting on it");
    }
}
