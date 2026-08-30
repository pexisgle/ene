//! Separate process that records pointer clicks. Used to prove OS click-through.

#![expect(clippy::print_stdout, reason = "click-sink logs hits for Experiment D")]
#![expect(clippy::print_stderr, reason = "fatal error path for a PoC binary")]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

fn main() {
    let log_path = std::env::var("ENE_STAGE_POC_SINK_LOG").map_or_else(
        |_| PathBuf::from("/tmp/ene-stage-poc-sink.log"),
        PathBuf::from,
    );
    if let Err(err) = run(log_path) {
        eprintln!("ene-stage-poc-click-sink: {err}");
        std::process::exit(1);
    }
}

fn run(log_path: PathBuf) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|err| err.to_string())?;
    let mut app = SinkApp {
        window: None,
        log_path,
        started: Instant::now(),
        clicks: 0,
        deadline: std::env::var("ENE_STAGE_POC_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(std::time::Duration::from_secs),
    };
    event_loop.run_app(&mut app).map_err(|err| err.to_string())
}

struct SinkApp {
    window: Option<Window>,
    log_path: PathBuf,
    started: Instant,
    clicks: u32,
    deadline: Option<std::time::Duration>,
}

impl ApplicationHandler for SinkApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("ene-stage-poc-click-sink")
            .with_inner_size(PhysicalSize::new(1000, 700))
            .with_position(PhysicalPosition::new(0, 0));
        match event_loop.create_window(attrs) {
            Ok(window) => {
                println!("SINK ready path={}", self.log_path.display());
                self.window = Some(window);
            }
            Err(err) => {
                eprintln!("SINK window failed: {err}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                self.clicks = self.clicks.saturating_add(1);
                let line = format!(
                    "SINK CLICK n={} wall_ms={:.0}\n",
                    self.clicks,
                    self.started.elapsed().as_secs_f64() * 1000.0
                );
                print!("{line}");
                if let Ok(mut file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.log_path)
                {
                    drop(file.write_all(line.as_bytes()));
                }
            }
            _ => {}
        }
        if self.deadline.is_some_and(|d| self.started.elapsed() >= d) {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.deadline.is_some_and(|d| self.started.elapsed() >= d) {
            event_loop.exit();
        }
    }
}
