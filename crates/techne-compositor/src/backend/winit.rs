//! Nested backend: the compositor in a window of another Wayland or X11
//! session, for development (EWM had none; it ran only on a TTY).
//!
//! One output, named `winit`, has the window's size. Input arrives as the
//! window's events and goes through the same handlers as libinput's. There is
//! no vblank: a submitted frame leaves the output idle and the next redraw is
//! queued by whatever changes, as on the headless backend.

use std::time::Duration;

use smithay::backend::input::{Event, InputEvent, KeyboardKeyEvent};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent, WinitGraphicsBackend, WinitInput};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::{EventLoop, LoopHandle};
use smithay::reexports::wayland_server::Display;
use smithay::utils::Scale;
use tracing::{info, warn};

use super::{
    Backend, OutputInfos, RenderResult, apply_enabled_output_config, apply_initial_output_config,
    map_initial_output,
};
use crate::cursor::{CursorConfig, CursorTextureCache};
use crate::render::{collect_render_elements_for_output, process_screencopies_for_output};
use crate::{Ewm, OutputState, RedrawState, State};

/// The output's name.
pub const OUTPUT_NAME: &str = "winit";

pub struct WinitBackend {
    backend: WinitGraphicsBackend<GlesRenderer>,
    output: Output,
    damage_tracker: OutputDamageTracker,
    cursor_texture_cache: CursorTextureCache,
    output_infos: OutputInfos,
    loop_handle: LoopHandle<'static, State>,
}

impl WinitBackend {
    pub(crate) fn output_infos(&self) -> OutputInfos {
        self.output_infos.clone()
    }

    fn mode(&self) -> Mode {
        Mode {
            size: self.backend.window_size(),
            refresh: 60_000,
        }
    }

    /// Create the window's output in `ewm`.
    fn add_output(&mut self, ewm: &mut Ewm) {
        let mode = self.mode();
        let output = &self.output;
        let config = ewm.output_config.get(OUTPUT_NAME).cloned();
        apply_initial_output_config(output, mode, config.as_ref());
        output.set_preferred(mode);
        output.create_global::<State>(&ewm.display_handle);
        let (position, _) = map_initial_output(ewm, output, config.as_ref());
        let (width, height) = (mode.size.w, mode.size.h);
        ewm.output_state.insert(
            output.clone(),
            OutputState::new(
                OUTPUT_NAME,
                Some(Duration::from_micros(16_667)),
                (width, height),
            ),
        );
        let info = crate::OutputInfo {
            name: OUTPUT_NAME.to_string(),
            make: "Techne".to_string(),
            model: "Nested".to_string(),
            serial: String::new(),
            width_mm: 0,
            height_mm: 0,
            x: position.x,
            y: position.y,
            scale: output.current_scale().fractional_scale(),
            transform: super::transform_to_int(output.current_transform()),
            modes: vec![crate::OutputMode {
                width,
                height,
                refresh: 60_000,
                preferred: true,
                current: true,
            }],
        };
        self.output_infos.insert(info.clone());
        ewm.add_output(output, info);
        info!("Nested output {OUTPUT_NAME}: {width}x{height}");
    }

    /// Apply the stored configuration (scale, transform, position); the
    /// mode stays the window's size.
    pub(crate) fn apply_output_config(&mut self, ewm: &mut Ewm, output_name: &str) {
        if output_name == OUTPUT_NAME {
            self.resized(ewm);
        }
    }

    /// The window was resized: the output takes its new size.
    fn resized(&mut self, ewm: &mut Ewm) {
        let mode = self.mode();
        let config = ewm
            .output_config
            .get(OUTPUT_NAME)
            .cloned()
            .unwrap_or(crate::OutputConfig {
                mode: None,
                modeline: None,
                position: None,
                scale: None,
                transform: None,
                enabled: true,
            });
        apply_enabled_output_config(ewm, &self.output_infos, &self.output, &config, Some(mode));
        ewm.queue_redraw(&self.output.clone());
    }

    pub(crate) fn render(&mut self, ewm: &mut Ewm, output: &Output) -> RenderResult {
        if *output != self.output {
            return RenderResult::Skipped;
        }
        let geometry = ewm.space.output_geometry(output).unwrap_or_default();
        let scale = Scale::from(output.current_scale().fractional_scale());
        let (mut elements, cursor) = collect_render_elements_for_output(
            ewm,
            self.backend.renderer(),
            scale,
            &self.cursor_texture_cache,
            geometry.loc,
            geometry.size,
            true,
            true,
            output,
        );
        elements.splice(0..0, cursor);

        let age = self.backend.buffer_age().unwrap_or(0);
        let damage = match self.backend.bind() {
            Ok((renderer, mut framebuffer)) => {
                match self.damage_tracker.render_output(
                    renderer,
                    &mut framebuffer,
                    age,
                    &elements,
                    [0.1, 0.1, 0.1, 1.0],
                ) {
                    Ok(result) => result.damage.cloned(),
                    Err(err) => {
                        warn!("nested render failed: {err:?}");
                        return RenderResult::Skipped;
                    }
                }
            }
            Err(err) => {
                warn!("nested bind failed: {err:?}");
                return RenderResult::Skipped;
            }
        };
        let submitted = match damage {
            Some(damage) => match self.backend.submit(Some(&damage)) {
                Ok(()) => true,
                Err(err) => {
                    warn!("nested submit failed: {err:?}");
                    return RenderResult::Skipped;
                }
            },
            None => false,
        };
        if let Some(state) = ewm.output_state.get_mut(output) {
            state.redraw_state = RedrawState::Idle;
            if submitted {
                state.frame_callback_sequence = state.frame_callback_sequence.wrapping_add(1);
            }
        }
        if submitted {
            RenderResult::Submitted
        } else {
            RenderResult::NoDamage
        }
    }

    pub(crate) fn post_render(&mut self, ewm: &mut Ewm, output: &Output) {
        if ewm.screencopy_state.has_pending_for_output(output) {
            process_screencopies_for_output(
                ewm,
                self.backend.renderer(),
                output,
                &self.cursor_texture_cache,
                &self.loop_handle,
            );
        }
    }

    pub(crate) fn with_renderer<F>(&mut self, f: F)
    where
        F: FnOnce(&mut GlesRenderer, &CursorTextureCache, &LoopHandle<'static, State>),
    {
        f(
            self.backend.renderer(),
            &self.cursor_texture_cache,
            &self.loop_handle,
        );
    }

    pub(crate) fn clear_cursor_texture_cache(&mut self) {
        self.cursor_texture_cache = CursorTextureCache::default();
    }
}

fn dispatch_input(state: &mut State, event: InputEvent<WinitInput>) {
    use crate::input::*;
    match event {
        InputEvent::Keyboard { event } => {
            // Nested: there are no VTs to switch to.
            let _ = handle_keyboard_event(
                state,
                event.key_code().into(),
                event.state(),
                Event::time(&event),
            );
        }
        InputEvent::PointerMotionAbsolute { event } => {
            handle_pointer_motion_absolute::<WinitInput>(state, event);
            state.ewm.queue_redraw_for_pointer();
        }
        InputEvent::PointerButton { event } => handle_pointer_button::<WinitInput>(state, event),
        InputEvent::PointerAxis { event } => handle_pointer_axis::<WinitInput>(state, event),
        _ => {}
    }
}

/// Run the compositor in a window until it is closed or stopped.
pub fn run_winit(cursor_config: CursorConfig) -> Result<(), Box<dyn std::error::Error>> {
    info!("Starting the compositor nested in a window");
    cursor_config.apply_env();
    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let display: Display<State> = Display::new()?;
    let display_handle = display.handle();
    display_handle.set_default_max_buffer_size(1024 * 1024);

    // The window's own connection uses the parent session's WAYLAND_DISPLAY;
    // create it before our socket replaces that variable.
    let (graphics, winit_loop) = winit::init::<GlesRenderer>()?;

    let socket_name = Ewm::init_wayland_listener(display, &event_loop.handle())?;
    let socket_name = socket_name.to_string_lossy().to_string();
    info!("Wayland socket: {socket_name}");

    let mut ewm = Ewm::new(
        display_handle.clone(),
        event_loop.handle(),
        false,
        cursor_config.clone(),
    );
    ewm.connect_im_relay();

    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Techne".into(),
            model: "Nested".into(),
            serial_number: String::new(),
        },
    );
    let damage_tracker = OutputDamageTracker::from_output(&output);
    let mut backend = WinitBackend {
        backend: graphics,
        output,
        damage_tracker,
        cursor_texture_cache: CursorTextureCache::default(),
        output_infos: OutputInfos::default(),
        loop_handle: event_loop.handle(),
    };
    backend.add_output(&mut ewm);
    let mut state = State {
        backend: Backend::Winit(Box::new(backend)),
        ewm,
    };

    let mut env_vars = std::collections::HashMap::<String, String>::from([
        ("WAYLAND_DISPLAY".into(), socket_name),
        ("XDG_SESSION_TYPE".into(), "wayland".into()),
    ]);
    env_vars.extend(cursor_config.env_vars());
    // SAFETY: still single-threaded at this point
    unsafe {
        std::env::remove_var("DISPLAY");
        for (k, v) in &env_vars {
            std::env::set_var(k, v);
        }
    }
    state
        .ewm
        .queue_event(crate::event::Event::Environment { vars: env_vars });

    event_loop
        .handle()
        .insert_source(winit_loop, |event, _, state| match event {
            WinitEvent::Resized { .. } => {
                if let Backend::Winit(winit) = &mut state.backend {
                    winit.resized(&mut state.ewm);
                }
            }
            WinitEvent::Input(event) => dispatch_input(state, event),
            WinitEvent::Redraw => {
                if let Backend::Winit(winit) = &state.backend {
                    let output = winit.output.clone();
                    state.ewm.queue_redraw(&output);
                }
            }
            WinitEvent::CloseRequested => state.ewm.stop(),
            WinitEvent::Focus(_) => {}
        })
        .map_err(|e| format!("winit event source: {e}"))?;

    let _ = crate::policy::LOOP_SIGNAL.set(event_loop.get_signal());
    state.ewm.queue_event(crate::event::Event::Ready);
    info!("Compositor ready");

    event_loop
        .run(None, &mut state, |state| state.refresh_and_flush_clients())
        .map_err(|e| format!("Event loop error: {e:?}"))?;
    info!("Nested compositor shutting down");
    Ok(())
}
