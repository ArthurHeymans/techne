//! Drawing a frame with wgpu: rectangles (highlights, selections, carets,
//! mode lines) from a small instanced pipeline, images (embedded blocks)
//! as textured quads, then text through glyphon's glyph atlas. Text of another colour in a line (a highlight's) is the
//! line's text drawn in spans, each clipped to its own.

use std::{collections::HashMap, sync::Arc};

use techne_editor::{
    images::{Image, encode_png},
    present::CapturedFrame,
};

use glyphon::{Cache, Color, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport};
use wgpu::util::DeviceExt;
use winit::window::Window;

use glyphon::FontSystem;

use crate::layout::Rect;

/// A colour as sRGB bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    fn linear(self) -> [f32; 4] {
        let f = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        [f(self.0), f(self.1), f(self.2), 1.0]
    }
    fn glyphon(self) -> Color {
        Color::rgb(self.0, self.1, self.2)
    }
}

/// Drawn where a style says no colour and nothing under it does
/// either: until the first snapshot, or for a `default' face a theme
/// left unset. Everything else takes its colours from the snapshot's
/// styles (the faces Lisp defined).
pub const FALLBACK_BG: Rgb = Rgb(0x1e, 0x1e, 0x2e);
pub const FALLBACK_FG: Rgb = Rgb(0xcd, 0xd6, 0xf4);

/// A line from `left` to `right` in spans of one colour: `plain`, with the
/// `colored` ranges (from, to, colour) over it in order.
pub fn spans(left: f32, right: f32, colored: &[(f32, f32, Rgb)], plain: Rgb) -> Vec<(f32, f32, Rgb)> {
    colored.iter().fold(vec![(left, right, plain)], |spans, &(from, to, color)| {
        spans
            .into_iter()
            .flat_map(|(a, b, c)| [(a, b.min(from), c), (a.max(from), b.min(to), color), (a.max(to), b, c)])
            .filter(|(a, b, _)| a < b)
            .fold(Vec::new(), |mut spans: Vec<(f32, f32, Rgb)>, (a, b, c)| {
                match spans.last_mut() {
                    Some(last) if last.2 == c => last.1 = b,
                    _ => spans.push((a, b, c)),
                }
                spans
            })
    })
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    rect: [f32; 4],
    color: [f32; 4],
}

/// One piece of text to draw: a shaped buffer from the layout cache (or the
/// status line) at a position, clipped to a box.
pub struct TextPiece<'a> {
    pub buffer: &'a glyphon::Buffer,
    pub left: f32,
    pub top: f32,
    pub clip: Rect,
    pub color: Rgb,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    rect_pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    swash: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text: TextRenderer,
    image_pipeline: wgpu::RenderPipeline,
    image_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Each image's texture by its identity, the frame it was last drawn
    /// in, and its size in bytes. Kept while they fit `TEXTURE_BUDGET`, so
    /// an image scrolled out and back is not converted and uploaded again;
    /// past it, the least recently drawn go.
    textures: HashMap<u64, (wgpu::BindGroup, u64, usize)>,
    texture_bytes: usize,
    frames: u64,
    pub captured: Option<Result<CapturedFrame, String>>,
    // Dropped last: the surface refers to it.
    pub window: Arc<Window>,
}

/// The bytes of image textures kept for images not drawn this frame.
const TEXTURE_BUDGET: usize = 256 << 20;

const SHADER: &str = r#"
struct Uniforms { size: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32, @location(0) rect: vec4<f32>, @location(1) color: vec4<f32>) -> Out {
    var corners = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let p = rect.xy + corners[i] * rect.zw;
    var o: Out;
    o.pos = vec4(p.x / u.size.x * 2.0 - 1.0, 1.0 - p.y / u.size.y * 2.0, 0.0, 1.0);
    o.color = color;
    return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> { return i.color; }
"#;

/// An image's quad: its rectangle from the vertex instance, its pixels
/// from the texture, premultiplied in linear light (`linear_premultiplied`):
/// filtered as they are, and blended so.
const IMAGE_SHADER: &str = r#"
struct Uniforms { size: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32, @location(0) rect: vec4<f32>) -> Out {
    var corners = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let p = rect.xy + corners[i] * rect.zw;
    var o: Out;
    o.pos = vec4(p.x / u.size.x * 2.0 - 1.0, 1.0 - p.y / u.size.y * 2.0, 0.0, 1.0);
    o.uv = corners[i];
    return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> { return textureSample(tex, samp, i.uv); }
"#;

impl Renderer {
    pub fn new(window: Arc<Window>, display: winit::event_loop::OwnedDisplayHandle) -> Renderer {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(display)));
        let surface = instance.create_surface(window.clone()).expect("a surface for the window");
        let adapter = pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() }),
        )
        .expect("a GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a GPU device");
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            // Mailbox when available: a frame as soon as it is ready.
            present_mode: if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
                wgpu::PresentMode::Mailbox
            } else {
                wgpu::PresentMode::Fifo
            },
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let shader = device
            .create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("rects"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rect uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rects"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Images: the rectangles' uniforms, and a texture and sampler each.
        let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("images"),
            source: wgpu::ShaderSource::Wgsl(IMAGE_SHADER.into()),
        });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let image_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bind_layout), Some(&image_layout)],
            immediate_size: 0,
        });
        let image_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("images"),
            layout: Some(&image_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &image_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &image_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("images"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        Renderer {
            surface,
            device,
            queue,
            config,
            rect_pipeline,
            uniforms,
            bind_group,
            swash: SwashCache::new(),
            viewport,
            atlas,
            text,
            image_pipeline,
            image_layout,
            sampler,
            textures: HashMap::new(),
            texture_bytes: 0,
            frames: 0,
            captured: None,
            window,
        }
    }

    /// The bind group of `image`'s texture, made the first time it is drawn.
    fn texture(&mut self, image: &Image) -> &wgpu::BindGroup {
        let frame = self.frames;
        let (device, queue, layout, sampler) = (&self.device, &self.queue, &self.image_layout, &self.sampler);
        let texture_bytes = &mut self.texture_bytes;
        let entry = self.textures.entry(image.id()).or_insert_with(|| {
            let (w, h) = image.size();
            let bytes = 8 * w as usize * h as usize;
            *texture_bytes += bytes;
            let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("image"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&linear_premultiplied(&image.0.rgba)),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8 * w), rows_per_image: Some(h) },
                size,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("image"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            });
            (group, frame, bytes)
        });
        entry.1 = frame;
        &entry.0
    }

    /// Drop the least recently drawn textures until the rest fit
    /// `TEXTURE_BUDGET`; those drawn this frame stay whatever they weigh.
    fn evict_textures(&mut self) {
        if self.texture_bytes <= TEXTURE_BUDGET {
            return;
        }
        let mut old: Vec<(u64, u64, usize)> =
            self.textures.iter().filter(|(_, e)| e.1 < self.frames).map(|(&id, e)| (e.1, id, e.2)).collect();
        old.sort_unstable();
        for (_, id, bytes) in old {
            if self.texture_bytes <= TEXTURE_BUDGET {
                break;
            }
            self.textures.remove(&id);
            self.texture_bytes -= bytes;
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub fn size(&self) -> (f32, f32) {
        (self.config.width as f32, self.config.height as f32)
    }

    /// Draw rectangles, then images (each where it is, clipped to the
    /// part shown), then text, on `background`. Returns false when the
    /// surface was not ready and the frame should be drawn again.
    pub fn draw(
        &mut self,
        fonts: &mut FontSystem,
        background: Rgb,
        rects: &[(Rect, Rgb)],
        images: &[(Rect, Rect, Image)],
        texts: &[TextPiece],
        capture: bool,
    ) -> bool {
        self.captured = None;
        self.frames += 1;
        for (_, _, image) in images {
            self.texture(image);
        }
        self.evict_textures();
        let quads: Vec<[f32; 4]> = images.iter().map(|(r, _, _)| [r.x, r.y, r.w, r.h]).collect();
        let quad_buffer = (!quads.is_empty()).then(|| {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("image quads"),
                contents: bytemuck::cast_slice(&quads),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let (w, h) = self.size();
        self.viewport.update(&self.queue, Resolution { width: self.config.width, height: self.config.height });
        self.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[w, h, 0.0, 0.0]));
        let areas = texts.iter().map(|t| TextArea {
            buffer: t.buffer,
            left: t.left,
            top: t.top,
            scale: 1.0,
            bounds: TextBounds {
                left: t.clip.x as i32,
                top: t.clip.y as i32,
                right: (t.clip.x + t.clip.w) as i32,
                bottom: (t.clip.y + t.clip.h) as i32,
            },
            default_color: t.color.glyphon(),
            custom_glyphs: &[],
        });
        if let Err(e) = self.text.prepare(&self.device, &self.queue, fonts, &mut self.atlas, &self.viewport, areas, &mut self.swash) {
            eprintln!("techne: preparing text: {e}");
        }
        let instances: Vec<Instance> = rects.iter().map(|(r, c)| Instance { rect: [r.x, r.y, r.w, r.h], color: c.linear() }).collect();
        let instance_buffer = (!instances.is_empty()).then(|| {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rect instances"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(_) | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            _ => return false,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Reuse exactly the same render pass for the capture. Surface textures
        // need not support COPY_SRC on every backend; an offscreen attachment
        // does. No second layout, compositor or approximation of the scene.
        if capture {
            if !matches!(
                self.config.format,
                wgpu::TextureFormat::Bgra8Unorm
                    | wgpu::TextureFormat::Bgra8UnormSrgb
                    | wgpu::TextureFormat::Rgba8Unorm
                    | wgpu::TextureFormat::Rgba8UnormSrgb
            ) {
                self.captured = Some(Err("capture needs an RGBA8 or BGRA8 surface".into()));
            } else if self.config.width as u64 * self.config.height as u64 > 16_777_216 {
                self.captured = Some(Err("capture exceeds the 16 megapixel limit".into()));
            }
        }
        let capture = capture && self.captured.is_none();
        let offscreen = capture.then(|| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("capture"),
                size: wgpu::Extent3d { width: self.config.width, height: self.config.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        });
        let capture_view = offscreen.as_ref().map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        for target in std::iter::once(&view).chain(capture_view.as_ref()) {
            let bg = background.linear();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: bg[0] as f64, g: bg[1] as f64, b: bg[2] as f64, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(buf) = &instance_buffer {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..6, 0..instances.len() as u32);
            }
            if let Some(buf) = &quad_buffer {
                pass.set_pipeline(&self.image_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                let (fw, fh) = (self.config.width as f32, self.config.height as f32);
                for (i, (_, clip, image)) in images.iter().enumerate() {
                    let (x, y) = (clip.x.clamp(0.0, fw), clip.y.clamp(0.0, fh));
                    let (w, h) = ((clip.x + clip.w).clamp(0.0, fw) - x, (clip.y + clip.h).clamp(0.0, fh) - y);
                    if w < 1.0 || h < 1.0 {
                        continue;
                    }
                    pass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
                    pass.set_bind_group(1, &self.textures[&image.id()].0, &[]);
                    pass.draw(0..6, i as u32..i as u32 + 1);
                }
                pass.set_scissor_rect(0, 0, self.config.width, self.config.height);
            }
            if let Err(e) = self.text.render(&self.atlas, &self.viewport, &mut pass) {
                eprintln!("techne: drawing text: {e}");
            }
        }
        let padded = (self.config.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = offscreen.as_ref().map(|texture| {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("capture readback"),
                size: padded as u64 * self.config.height as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(self.config.height),
                    },
                },
                texture.size(),
            );
            buffer
        });
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        if let Some(buffer) = readback {
            self.captured = Some(self.read_capture(&buffer, padded));
        }
        self.atlas.trim();
        true
    }

    fn read_capture(&self, buffer: &wgpu::Buffer, padded: u32) -> Result<CapturedFrame, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(5)) })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(std::time::Duration::from_secs(1)).map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        let bytes = buffer.slice(..).get_mapped_range().map_err(|e| e.to_string())?;
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut rgba = Vec::with_capacity((self.config.width as usize * self.config.height as usize) * 4);
        for row in bytes.chunks_exact(padded as usize) {
            rgba.extend_from_slice(&row[..self.config.width as usize * 4]);
        }
        drop(bytes);
        buffer.unmap();
        if bgra {
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        let png = encode_png(self.config.width, self.config.height, &rgba)?;
        Ok(CapturedFrame { snapshot: 0, width: self.config.width, height: self.config.height, png })
    }
}

/// sRGB pixels with straight alpha as half floats premultiplied in
/// linear light: what a sampler may interpolate and the blending takes.
/// Premultiplied after filtering, a transparent pixel's black would
/// darken the edges it is filtered with.
pub fn linear_premultiplied(rgba: &[u8]) -> Vec<u16> {
    let linear: Vec<f32> = (0..256)
        .map(|c| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        })
        .collect();
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as f32 / 255.0;
            [half(linear[p[0] as usize] * a), half(linear[p[1] as usize] * a), half(linear[p[2] as usize] * a), half(a)]
        })
        .collect()
}

/// A float from 0 to 1 as an IEEE half float, rounded to nearest.
fn half(x: f32) -> u16 {
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mant = bits & 0x7f_ffff;
    if exp <= 0 {
        // Subnormal, or too small: zero.
        if exp < -10 {
            return 0;
        }
        let m = (mant | 0x80_0000) >> (1 - exp);
        return ((m + 0x1000) >> 13) as u16;
    }
    // A carry out of the mantissa rounds up into the exponent, as it should.
    (((exp as u32) << 10) + ((mant + 0x1000) >> 13)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_premultiplied_in_linear_light() {
        // Opaque white, half-transparent white, transparent black.
        let out = linear_premultiplied(&[255, 255, 255, 255, 255, 255, 255, 128, 0, 0, 0, 0]);
        let one = 0x3c00;
        assert_eq!(&out[..4], [one; 4]);
        // Half of linear white, alpha a half: so filtering it with the
        // opaque white gives white, not grey.
        let h = half(128.0 / 255.0);
        assert_eq!(&out[4..8], [h, h, h, h]);
        assert_eq!(&out[8..], [0; 4]);
        assert_eq!(half(0.5), 0x3800);
        assert_eq!(half(0.25), 0x3400);
    }

    #[test]
    fn a_line_in_spans() {
        let (a, b) = (Rgb(1, 0, 0), Rgb(0, 2, 0));
        let p = Rgb(9, 9, 9);
        assert_eq!(spans(0.0, 100.0, &[], p), [(0.0, 100.0, p)]);
        // Overlapping (the later over the earlier), one past the right edge.
        let colored = [(10.0, 20.0, a), (50.0, 70.0, b), (60.0, 80.0, a), (90.0, 120.0, b)];
        let expected =
            [(0.0, 10.0, p), (10.0, 20.0, a), (20.0, 50.0, p), (50.0, 60.0, b), (60.0, 80.0, a), (80.0, 90.0, p), (90.0, 100.0, b)];
        assert_eq!(spans(0.0, 100.0, &colored, p), expected);
    }

    #[test]
    fn srgb_bytes_in_linear_light() {
        assert_eq!(Rgb(0, 0, 0).linear(), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(Rgb(255, 255, 255).linear(), [1.0, 1.0, 1.0, 1.0]);
        // The sRGB curve: 128 is about 0.216 in linear light, and the
        // dark end is the straight segment (10/255 under 0.04045).
        let mid = Rgb(128, 0, 10).linear();
        assert!((mid[0] - 0.2158).abs() < 0.001, "{mid:?}");
        assert!((mid[2] - (10.0 / 255.0) / 12.92).abs() < 1e-6, "{mid:?}");
    }
}
