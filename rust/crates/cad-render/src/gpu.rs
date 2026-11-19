//! Headless GPU access (tests, `--thumbnail`, screenshots without a window) and small blocking helpers.
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use eframe::wgpu;

/// Errors of the renderer. Messages are for logs; user-visible Czech strings belong to the application.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("no suitable GPU adapter: {0}")]
    NoAdapter(String),
    #[error("GPU device request failed: {0}")]
    Device(String),
    #[error("GPU read-back failed: {0}")]
    ReadBack(String),
    #[error("invalid image size {0}x{1}")]
    BadSize(u32, u32),
    #[error("PNG encoding failed: {0}")]
    Png(String),
    #[error("no queue attached to the renderer (call with_queue or paint once)")]
    NoQueue,
    #[error("nothing to draw")]
    Empty,
}

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Minimal executor for the few futures wgpu hands out (adapter / device request, error scopes).
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match Pin::as_mut(&mut fut).poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => thread::park(),
        }
    }
}

/// An instance, adapter, device and queue created without a window.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    /// Hardware adapter first, then the software fallback adapter (WARP on Windows, lavapipe on Linux).
    pub fn new_headless() -> Result<Self, RenderError> {
        match Self::request(false) {
            Ok(c) => Ok(c),
            Err(first) => Self::request(true).map_err(|second| {
                RenderError::NoAdapter(format!("hardware: {first}; fallback: {second}"))
            }),
        }
    }

    /// Only the software fallback adapter (`--thumbnail` in sandboxes without a GPU).
    pub fn new_software() -> Result<Self, RenderError> {
        Self::request(true)
    }

    fn request(fallback: bool) -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: fallback,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cad-render headless"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| RenderError::Device(e.to_string()))?;
        Ok(Self { instance, adapter, device, queue })
    }

    /// One line for logs: adapter name, backend and device type.
    pub fn describe(&self) -> String {
        let i = self.adapter.get_info();
        format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_runs_ready_future() {
        assert_eq!(block_on(async { 21 * 2 }), 42);
    }

    #[test]
    fn adapter_probe() {
        match GpuContext::new_headless() {
            Ok(g) => println!("GPU adapter: {}", g.describe()),
            Err(e) => println!("SKIP adapter_probe: {e}"),
        }
    }
}
