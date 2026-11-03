//! Images for frontends to draw: decoded once, off the editor's thread,
//! and shown by any mode as embedded blocks (EDITOR.md, sections 2 and 3:
//! a projection, `(block AT IMAGE)`). Decoding is Rust's (the `image`
//! crate's raster formats, `resvg` for SVG): a format parser in Scheme
//! would be far too slow for a picture, and the pixels go to the GPU.
//!
//! An image is its pixels, premultiplied RGBA in sRGB, with an identity
//! frontends cache textures by. Pictures larger than `MAX_SIDE` on a side
//! are scaled down as they are decoded: a frontend draws them smaller
//! than that anyway. Lisp loads images by path (`image-load`, which the
//! calling task waits for while a thread decodes), and a loaded image is
//! kept by path and modification time, so a view rendered again does not
//! decode again.

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

/// How many bytes of decoded images the cache keeps, past which the
/// least recently loaded go (those still shown stay alive where shown).
const CACHE_BYTES: usize = 256 << 20;

/// An image's pixels.
pub struct ImageData {
    /// Never reused within the process: frontends key textures by it.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA, row by row, in sRGB.
    pub rgba: Vec<u8>,
}

/// A decoded image, shared: cheap to clone into snapshots.
#[derive(Clone)]
pub struct Image(pub Arc<ImageData>);

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

/// Decode the image in `bytes`, from a file named `name`.
pub fn decode(name: &Path, bytes: &[u8]) -> Result<Image, String> {
    if is_svg(name, bytes) {
        let options = resvg::usvg::Options { fontdb: fonts(), resources_dir: name.parent().map(Path::to_path_buf), ..Default::default() };
        let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
        let size = tree.size();
        let (w, h, scale) = fitted(size.width(), size.height());
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("an empty SVG")?;
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        // tiny-skia's pixels are premultiplied RGBA already.
        return Ok(image(w, h, pixmap.take()));
    }
    let mut decoded = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let (w, h, _) = fitted(decoded.width() as f32, decoded.height() as f32);
    if (w, h) != (decoded.width(), decoded.height()) {
        decoded = decoded.resize(w, h, image::imageops::FilterType::Triangle);
    }
    let rgba = decoded.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut pixels = rgba.into_raw();
    for p in pixels.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        if a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    Ok(image(w, h, pixels))
}

/// Decode the image file at `path`.
pub fn load(path: &Path) -> Result<Image, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
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

impl<T: Send + 'static> OnThread<T> {
    pub(crate) fn spawn(name: &str, work: impl FnOnce() -> T + Send + 'static) -> OnThread<T> {
        let shared = Arc::new(Mutex::new((None, None::<std::task::Waker>)));
        let theirs = shared.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let result = work();
                let mut slot = theirs.lock().expect("the result");
                slot.0 = Some(result);
                if let Some(w) = slot.1.take() {
                    w.wake();
                }
            })
            .expect("a worker thread starts");
        OnThread { shared }
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
                let image = OnThread::spawn("image-load", move || load(&theirs)).await?;
                cache.borrow_mut().insert(path, before, image.clone());
                Ok::<Img, String>(Foreign(Rc::new(image)))
            }
        });
        techne_vm::document! { vm;
            /// Return the image of the file at PATH, decoded on a thread of
            /// its own while the calling task waits: PNG, JPEG, GIF, WebP,
            /// BMP or SVG, scaled down to 4096 pixels a side at most. A
            /// file loaded before and not changed since is not decoded
            /// again. Show it with a projection, `(block AT IMAGE)`.
            "(image-load path)";
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasters_and_svg_decode_premultiplied() {
        // A 2x1 PNG: opaque red, then half-transparent white.
        let mut png = Vec::new();
        let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 255, 255, 255, 128]).unwrap();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let i = decode(Path::new("x.png"), &png).unwrap();
        assert_eq!(i.size(), (2, 1));
        assert_eq!(i.0.rgba, [255, 0, 0, 255, 128, 128, 128, 128]);
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
}
