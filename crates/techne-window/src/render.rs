//! Drawing a frame with wgpu: rectangles (selection, cursor, status bar)
//! from a small instanced pipeline, then text through glyphon's glyph atlas.

use std::sync::Arc;

use glyphon::{Cache, Color, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport};
use wgpu::util::DeviceExt;
use winit::window::Window;

use glyphon::FontSystem;

use crate::layout::Rect;

/// A colour as sRGB bytes.
#[derive(Clone, Copy)]
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

pub const BACKGROUND: Rgb = Rgb(0x1e, 0x1e, 0x2e);
pub const FOREGROUND: Rgb = Rgb(0xcd, 0xd6, 0xf4);
pub const SELECTION: Rgb = Rgb(0x45, 0x47, 0x5a);
pub const CURSOR: Rgb = Rgb(0xf5, 0xe0, 0xdc);
pub const STATUS_BG: Rgb = Rgb(0x31, 0x32, 0x44);

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
    // Dropped last: the surface refers to it.
    pub window: Arc<Window>,
}

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
            window,
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

    /// Draw rectangles, then text. Returns false when the surface was not
    /// ready and the frame should be drawn again.
    pub fn draw(&mut self, fonts: &mut FontSystem, rects: &[(Rect, Rgb)], texts: &[TextPiece]) -> bool {
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
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let bg = BACKGROUND.linear();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            if let Err(e) = self.text.render(&self.atlas, &self.viewport, &mut pass) {
                eprintln!("techne: drawing text: {e}");
            }
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.atlas.trim();
        true
    }
}
