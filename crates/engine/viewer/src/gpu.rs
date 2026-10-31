//! wgpu initialization, surface configuration and frame presentation.
//!
//! One [`Gpu`] owns the instance's device, queue, window surface, camera
//! uniform and depth buffer. The surface is configured once the window exists
//! and reconfigured whenever the drawable size changes. A zero-sized window
//! (minimized) is left alone: wgpu rejects a zero width or height, and the next
//! [`WindowEvent::Resized`] brings a usable size back.
//!
//! [`Gpu::render`] uploads the camera, pumps the streaming ring, acquires the
//! next frame, clears color and depth, draws every resident chunk and presents. The present waits on the
//! swap chain's `Fifo` mode, so the event loop runs at the display's refresh
//! rate rather than spinning.
//!
//! [`WindowEvent::Resized`]: winit::event::WindowEvent::Resized

use std::sync::Arc;

use pollster::block_on;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, Buffer, BufferBindingType, BufferDescriptor, BufferUsages,
    Color, CommandEncoderDescriptor, CurrentSurfaceTexture, Device, DeviceDescriptor, Instance,
    InstanceDescriptor, LoadOp, Operations, Queue, RenderPassColorAttachment,
    RenderPassDepthStencilAttachment, RenderPassDescriptor, RequestAdapterOptions, ShaderStages,
    StoreOp, Surface, SurfaceConfiguration, TextureViewDescriptor,
};
use winit::dpi::PhysicalSize;
use winit::event_loop::OwnedDisplayHandle;
use winit::window::Window;

use crate::atlas::Atlas;
use crate::atlas_texture::AtlasTexture;
use crate::camera::{Camera, CameraUniform};
use crate::chunk_lighting;
use crate::chunk_render::ChunkPipeline;
use crate::depth::DepthTarget;
use crate::error::ViewerError;
use crate::streaming::{self, PUMP_BUDGET, Streaming};

/// Consecutive `Lost` surfaces tolerated before the viewer gives up.
const MAX_SURFACE_RECREATIONS: u32 = 3;

/// The graphics handles a frame needs.
pub struct Gpu {
    /// The instance the surface was created from; kept so a lost surface can
    /// be recreated on the same adapter/device.
    instance: Instance,
    /// The window the surface targets.
    window: Arc<Window>,
    /// The logical device that owns every resource the viewer creates.
    pub device: Device,
    /// The queue that executes submissions and presents frame textures.
    pub queue: Queue,
    /// The presentation surface for the window.
    pub surface: Surface<'static>,
    /// The active surface configuration; rewritten on resize.
    pub config: SurfaceConfiguration,
    /// Consecutive `Lost` acquisitions since the last presented frame.
    lost_retries: u32,
    /// The camera uniform, rewritten every frame.
    pub camera_buffer: Buffer,
    /// Binding for [`Gpu::camera_buffer`].
    pub camera_bind_group: BindGroup,
    /// Depth attachment, recreated with the swap chain.
    pub depth: DepthTarget,
    /// The material atlas texture, sampler and bind group.
    pub atlas: AtlasTexture,
    /// The M3-01 chunk ring, its workers and its per-frame install budget.
    pub streaming: Streaming,
    /// The chunk draw pipeline.
    pub chunk_pipeline: ChunkPipeline,
}

impl Gpu {
    /// Creates the instance, adapter, device, queue and surface for `window`.
    ///
    /// The display handle comes from the event loop: GLES needs it on Wayland
    /// to create a surface at all.
    pub fn new(
        window: Arc<Window>,
        display: OwnedDisplayHandle,
        atlas: Atlas,
    ) -> Result<Self, ViewerError> {
        let instance = Instance::new(InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(display),
        ));
        let surface = instance.create_surface(window.clone())?;
        let adapter = block_on(instance.request_adapter(&RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let info = adapter.get_info();
        log::info!(
            "adapter: {} (backend {:?}, {:?})",
            info.name,
            info.backend,
            info.device_type
        );
        log::debug!("adapter details: {info:?}");

        let (device, queue) = block_on(adapter.request_device(&DeviceDescriptor::default()))?;
        // Log instead of panicking inside a winit callback when a validation
        // error slips through (for example a surface larger than the adapter's
        // limit).
        device.on_uncaptured_error(std::sync::Arc::new(|error: wgpu::Error| {
            log::error!("wgpu uncaptured error: {error}");
        }));

        let camera_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("camera"),
            layout: &camera_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let size = window.inner_size();
        // The adapter's limit applies to the first configure too; a HiDPI window
        // larger than it otherwise fails validation before the first frame.
        let max = device.limits().max_texture_dimension_2d;
        let config = surface
            .get_default_config(
                &adapter,
                size.width.max(1).min(max),
                size.height.max(1).min(max),
            )
            .ok_or(ViewerError::UnsupportedSurface)?;
        log::debug!("surface: {config:?}");
        surface.configure(&device, &config);

        let depth = DepthTarget::new(&device, config.width, config.height);
        let atlas = AtlasTexture::new(&device, &queue, &atlas);
        let data_layout = chunk_lighting::layout(&device);
        let chunk_pipeline = ChunkPipeline::new(
            &device,
            &camera_layout,
            &atlas.layout,
            &data_layout,
            config.format,
        );
        let streaming = Streaming::new(device.clone(), data_layout);
        log::info!(
            "streaming shell: {} chunk ring, {:.0} ms frame budget",
            streaming::RING_CHUNKS,
            PUMP_BUDGET.as_secs_f64() * 1000.0
        );

        Ok(Self {
            instance,
            window,
            device,
            queue,
            surface,
            config,
            lost_retries: 0,
            camera_buffer,
            camera_bind_group,
            depth,
            atlas,
            streaming,
            chunk_pipeline,
        })
    }

    /// Reconfigures the swap chain for a new drawable size.
    ///
    /// `size` is in physical pixels. Minimized windows report zero on one or
    /// both axes; those are ignored until the window is restored.
    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        // A drawable larger than the adapter's limit fails validation, so clamp
        // before configuring the surface.
        let max = self.device.limits().max_texture_dimension_2d;
        let width = size.width.min(max);
        let height = size.height.min(max);
        if width == self.config.width && height == self.config.height {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth = DepthTarget::new(&self.device, self.config.width, self.config.height);
    }

    /// Rebuilds the presentation surface and the depth target.
    ///
    /// Used when the surface is lost and when the platform recreates the
    /// window (resume): reconfiguring a dead surface is not enough.
    pub fn recreate_surface(&mut self) -> Result<(), ViewerError> {
        self.surface = self.instance.create_surface(self.window.clone())?;
        self.surface.configure(&self.device, &self.config);
        self.depth = DepthTarget::new(&self.device, self.config.width, self.config.height);
        Ok(())
    }

    /// Uploads `camera`, acquires the next surface texture, clears color and
    /// depth, draws every resident streamed chunk and presents.
    ///
    /// Returns `true` when a frame was presented. A skipped frame is not an
    /// error: occluded windows, timeouts and a swap chain that had to be
    /// reconfigured all return `Ok(false)`, so the caller can wait for the
    /// platform to request the next frame instead of spinning.
    pub fn render(&mut self, clear: Color, camera: &Camera) -> Result<bool, ViewerError> {
        let uniform = camera.uniform();
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        // Pump before acquiring the frame: the install budget is frame-thread
        // work, so it must happen before the frame is paced by presentation.
        let stats = self
            .streaming
            .pump(streaming::center_for(uniform.position), PUMP_BUDGET);
        log::debug!(
            "stream: {}/{} loaded, {} pending, {} installed, pump {:.3} ms (max {:.3} ms)",
            stats.loaded,
            streaming::RING_CHUNKS,
            stats.pending,
            stats.installed,
            stats.elapsed.as_secs_f64() * 1000.0,
            stats.max_elapsed.as_secs_f64() * 1000.0,
        );

        let frame = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => frame,
            CurrentSurfaceTexture::Suboptimal(frame) => {
                // A `SurfaceTexture` must not be alive across `configure`.
                drop(frame);
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
            CurrentSurfaceTexture::Lost => {
                // `Lost` means the surface itself is dead; reconfiguring it is
                // not enough. Rebuild it from the window and keep the device,
                // but bound the retries: a persistent loss would otherwise
                // livelock at the redraw cadence, rebuilding the surface (and
                // depth texture) forever.
                self.lost_retries += 1;
                if self.lost_retries > MAX_SURFACE_RECREATIONS {
                    return Err(ViewerError::SurfaceLost);
                }
                log::warn!(
                    "surface lost; recreating ({}/{MAX_SURFACE_RECREATIONS})",
                    self.lost_retries
                );
                self.recreate_surface()?;
                return Ok(false);
            }
            CurrentSurfaceTexture::Outdated => {
                log::warn!("surface is outdated; reconfiguring");
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => return Ok(false),
            CurrentSurfaceTexture::Validation => return Err(ViewerError::SurfaceValidation),
        };

        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("main frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("main frame"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(clear),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                    view: &self.depth.view,
                    depth_ops: Some(Operations {
                        load: LoadOp::Clear(1.0),
                        store: StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            self.streaming.draw(
                &mut pass,
                &self.chunk_pipeline,
                &self.camera_bind_group,
                &self.atlas.bind_group,
            );
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        self.lost_retries = 0;
        Ok(true)
    }
}
