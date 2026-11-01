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
use wgpu::util::StagingBelt;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, Buffer, BufferBindingType, BufferDescriptor, BufferSize,
    BufferUsages, Color, CommandEncoder, CommandEncoderDescriptor, CurrentSurfaceTexture, Device,
    DeviceDescriptor, Instance, InstanceDescriptor, LoadOp, Operations, Queue,
    RenderPassColorAttachment, RenderPassDepthStencilAttachment, RenderPassDescriptor,
    RequestAdapterOptions, ShaderStages, StoreOp, Surface, SurfaceConfiguration,
    TextureViewDescriptor,
};
use winit::dpi::PhysicalSize;
use winit::event_loop::OwnedDisplayHandle;
use winit::window::Window;

use crate::atlas::Atlas;
use crate::atlas_texture::AtlasTexture;
use crate::camera::{Camera, CameraUniform};
use crate::chunk_lighting;
use crate::chunk_render::ChunkPipeline;
use crate::cull::Frustum;
use crate::depth::DepthTarget;
use crate::error::ViewerError;
use crate::stats::{FrameStats, GpuPassTimer, StatsWindow};
use crate::streaming::{self, PUMP_BUDGET, PendingUpload, Streaming};
use crate::upload;

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
    /// M3-03 staging belt: copies finished meshes into the pool without a
    /// per-mesh driver allocation.
    upload_belt: StagingBelt,
    /// Mesh bytes one frame may copy into the pool.
    upload_budget: usize,
    /// M3-06 GPU pass timer; `None` when the adapter has no timestamp queries.
    timers: Option<GpuPassTimer>,
    /// M3-06 once-per-second frame summary.
    stats: StatsWindow,
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

        // M3-06 measures the render pass with in-pass timestamps when the
        // adapter has them; the device only gets features it asks for.
        let timestamp =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
        let mut required_features = wgpu::Features::empty();
        if adapter.features().contains(timestamp) {
            required_features |= timestamp;
        }
        let (device, queue) = block_on(adapter.request_device(&DeviceDescriptor {
            required_features,
            ..Default::default()
        }))?;
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
        let upload_budget = upload_budget_bytes();
        let streaming = Streaming::new(device.clone(), data_layout, upload_budget);
        let timers = GpuPassTimer::new(&device, &queue);
        let upload_belt = StagingBelt::new(device.clone(), upload::STAGING_CHUNK_BYTES);
        log::info!(
            "streaming shell: {} chunk ring, {:.0} ms frame budget, {:.0} KiB upload budget",
            streaming::RING_CHUNKS,
            PUMP_BUDGET.as_secs_f64() * 1000.0,
            upload_budget as f64 / 1024.0
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
            upload_belt,
            upload_budget,
            timers,
            stats: StatsWindow::new(),
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
            "stream: {}/{} loaded, {} pending, {} queued ({} MiB backlog), pump {:.3} ms (max {:.3} ms)",
            stats.loaded,
            streaming::RING_CHUNKS,
            stats.pending,
            stats.queued,
            self.streaming.upload_backlog_bytes() as f64 / (1024.0 * 1024.0),
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
        let cpu_started = std::time::Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("main frame"),
            });
        // M3-06: pick up the previous frame's GPU pass measurement before
        // issuing a new pair of timestamps.
        let gpu_pass = self
            .timers
            .as_mut()
            .and_then(|timer| timer.poll(&self.device));
        let measure_gpu = self.timers.as_ref().is_some_and(GpuPassTimer::ready);

        // M3-03: take at most the frame's upload budget, copy it through the
        // staging belt, then install the meshes so this frame's pass draws
        // them. The copies precede the pass in this encoder, so the GPU needs
        // no fence between them.
        let upload_started = std::time::Instant::now();
        let uploads = self.streaming.take_uploads(self.upload_budget);
        let upload_bytes: usize = uploads.iter().map(upload::UploadBytes::upload_bytes).sum();
        for upload in &uploads {
            stage_upload(&mut self.upload_belt, &mut encoder, upload);
        }
        let installed = self.streaming.install_uploads(uploads);
        let upload_elapsed = upload_started.elapsed();
        self.streaming
            .note_upload_frame(upload_bytes, upload_elapsed);
        if installed > 0 {
            log::trace!(
                "upload: {installed} meshes, {:.0} KiB in {:.3} ms",
                upload_bytes as f64 / 1024.0,
                upload_elapsed.as_secs_f64() * 1000.0,
            );
        }

        // M3-04: the stopgap broad phase. The cull is conservative, so the
        // image is identical to drawing the whole ring. M3-07 times this slice:
        // the frustum build plus recording every visible chunk's draw.
        let cull_started = std::time::Instant::now();
        let frustum = Frustum::from_view_projection(camera.view_projection());
        let draw_stats;
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
            if measure_gpu && let Some(timer) = &self.timers {
                timer.begin(&mut pass);
            }
            draw_stats = self.streaming.draw(
                &mut pass,
                &self.chunk_pipeline,
                &self.camera_bind_group,
                &self.atlas.bind_group,
                frustum,
            );
            if measure_gpu && let Some(timer) = &self.timers {
                timer.end(&mut pass);
            }
        }
        let cpu_cull = cull_started.elapsed();
        if measure_gpu && let Some(timer) = &self.timers {
            timer.resolve(&mut encoder);
        }
        // Unmaps the staging chunks and re-maps them when this submission
        // completes, so the next frame reuses the same buffers.
        let finish_started = std::time::Instant::now();
        self.upload_belt.finish_and_recall_on_submit(&encoder);
        self.queue.submit([encoder.finish()]);
        let cpu_finish = finish_started.elapsed();
        if measure_gpu && let Some(timer) = &mut self.timers {
            timer.submitted();
        }
        let cpu_submit = cpu_started.elapsed();
        self.queue.present(frame);

        // M3-06: one frame's counters, then a once-per-second summary line.
        let frame = FrameStats {
            resident: self.streaming.loaded(),
            visible: draw_stats.visible,
            drawn: draw_stats.drawn,
            blocks: draw_stats.blocks,
            quads: draw_stats.quads,
            mesh_bytes: draw_stats.bytes,
            upload_meshes: installed,
            upload_bytes,
            cpu_submit,
            cpu_upload: upload_elapsed,
            cpu_cull,
            cpu_finish,
            gpu_pass,
        };
        log::debug!(
            "frame: cpu {:.2} ms (cull {:.2}, upload {:.2}, finish {:.2}), gpu {}, {} visible / {} drawn of {}, {:.0} kquads, {:.1} MiB, upload {:.0} KiB",
            frame.cpu_submit.as_secs_f64() * 1000.0,
            frame.cpu_cull.as_secs_f64() * 1000.0,
            frame.cpu_upload.as_secs_f64() * 1000.0,
            frame.cpu_finish.as_secs_f64() * 1000.0,
            frame.gpu_pass.map_or_else(
                || "n/a".to_owned(),
                |gpu| format!("{:.2} ms", gpu.as_secs_f64() * 1000.0)
            ),
            frame.visible,
            frame.drawn,
            frame.resident,
            frame.quads as f64 / 1000.0,
            frame.mesh_bytes as f64 / (1024.0 * 1024.0),
            frame.upload_bytes as f64 / 1024.0,
        );
        if let Some(summary) = self.stats.record(frame) {
            log::info!("{}", summary.line());
        }
        self.lost_retries = 0;
        Ok(true)
    }
}

/// Defers one finished mesh into the staging belt and records the copy.
///
/// Both writes are padded to the 4 B copy alignment; the pool range already
/// reserves the pad, so the tail never leaves the mesh's page.
fn stage_upload(belt: &mut StagingBelt, encoder: &mut CommandEncoder, upload: &PendingUpload) {
    let Some(mesh) = &upload.mesh else {
        return;
    };
    let vertex_size = BufferSize::new(mesh.vertices.len).expect("a mesh has vertices");
    let mut view = belt.write_buffer(
        encoder,
        &mesh.vertices.buffer,
        mesh.vertices.offset,
        vertex_size,
    );
    view.copy_from_slice(&upload.vertices);
    drop(view);

    if let Some(indices) = &mesh.indices {
        let bytes = bytemuck::cast_slice(&upload.indices);
        let index_size = BufferSize::new(indices.len).expect("an indexed mesh has indices");
        let mut view = belt.write_buffer(encoder, &indices.buffer, indices.offset, index_size);
        view.slice(..bytes.len()).copy_from_slice(bytes);
        view.slice(bytes.len()..).fill(0);
        drop(view);
    }
}

/// Reads `VOXEL_UPLOAD_KIB`, clamped to a range that can still make progress.
fn upload_budget_bytes() -> usize {
    const MIN_KIB: usize = 64;
    const MAX_KIB: usize = 16 * 1024;
    std::env::var("VOXEL_UPLOAD_KIB")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|kib| kib.clamp(MIN_KIB, MAX_KIB) * 1024)
        .unwrap_or(upload::DEFAULT_UPLOAD_BYTES)
}
