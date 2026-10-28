//! wgpu initialization, surface configuration and frame presentation.
//!
//! One [`Gpu`] owns the instance's device, queue, window surface, camera
//! uniform and depth buffer. The surface is configured once the window exists
//! and reconfigured whenever the drawable size changes. A zero-sized window
//! (minimized) is left alone: wgpu rejects a zero width or height, and the next
//! [`WindowEvent::Resized`] brings a usable size back.
//!
//! [`Gpu::render`] uploads the camera, acquires the next frame, clears color
//! and depth, draws the test chunk and presents. The present waits on the
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
use crate::chunk_mesh::ChunkMesh;
use crate::chunk_render::ChunkPipeline;
use crate::depth::DepthTarget;
use crate::error::ViewerError;
use crate::naive_mesh::{Mesh, naive_mesh};
use voxel_core::constants::{CHUNK_BITS, CHUNK_VOLUME};

/// The graphics handles a frame needs.
pub struct Gpu {
    /// The logical device that owns every resource the viewer creates.
    pub device: Device,
    /// The queue that executes submissions and presents frame textures.
    pub queue: Queue,
    /// The presentation surface for the window.
    pub surface: Surface<'static>,
    /// The active surface configuration; rewritten on resize.
    pub config: SurfaceConfiguration,
    /// The camera uniform, rewritten every frame.
    pub camera_buffer: Buffer,
    /// Binding for [`Gpu::camera_buffer`].
    pub camera_bind_group: BindGroup,
    /// Depth attachment, recreated with the swap chain.
    pub depth: DepthTarget,
    /// The material atlas texture, sampler and bind group.
    pub atlas: AtlasTexture,
    /// One uploaded test chunk.
    pub chunk: Option<ChunkMesh>,
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
        let instance = Instance::new(InstanceDescriptor::new_with_display_handle(Box::new(
            display,
        )));
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
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or(ViewerError::UnsupportedSurface)?;
        log::debug!("surface: {config:?}");
        surface.configure(&device, &config);

        let depth = DepthTarget::new(&device, config.width, config.height);
        let atlas = AtlasTexture::new(&device, &queue, &atlas);
        let chunk_pipeline =
            ChunkPipeline::new(&device, &camera_layout, &atlas.layout, config.format);
        let chunk = ChunkMesh::new(&device, &queue, &test_chunk());
        if let Some(chunk) = &chunk {
            log::info!(
                "uploaded test chunk: {} quads, {} bytes",
                chunk.quad_count(),
                chunk.byte_len()
            );
        }

        Ok(Self {
            device,
            queue,
            surface,
            config,
            camera_buffer,
            camera_bind_group,
            depth,
            atlas,
            chunk,
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
        if size.width == self.config.width && size.height == self.config.height {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.depth = DepthTarget::new(&self.device, self.config.width, self.config.height);
    }

    /// Uploads `camera`, acquires the next surface texture, clears color and
    /// depth, draws the test chunk and presents.
    ///
    /// A skipped frame is not an error: occluded windows, timeouts and a swap
    /// chain that had to be reconfigured all return `Ok(())` so the loop can
    /// try again next frame.
    pub fn render(&self, clear: Color, camera: &Camera) -> Result<(), ViewerError> {
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&camera.uniform()),
        );

        let frame = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => frame,
            CurrentSurfaceTexture::Suboptimal(frame) => {
                // A `SurfaceTexture` must not be alive across `configure`.
                drop(frame);
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                log::warn!("surface is outdated or lost; reconfiguring");
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => return Ok(()),
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
            if let Some(chunk) = &self.chunk {
                self.chunk_pipeline.draw(
                    &mut pass,
                    chunk,
                    &self.camera_bind_group,
                    &self.atlas.bind_group,
                );
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        Ok(())
    }
}

/// Builds a deterministic terrain patch inside one chunk.
///
/// The patch is 48x48 columns centred in the 64x64 chunk, so the naive
/// whole-chunk mesh stays below the u16 index limit while still showing
/// slopes, material changes and multiple block faces.
fn test_chunk() -> Mesh {
    let mut volume = vec![0u16; CHUNK_VOLUME];
    for z in 8..56 {
        for x in 8..56 {
            let fx = (x as f32 - 32.0) / 16.0;
            let fz = (z as f32 - 32.0) / 16.0;
            let mound = (1.0 - (fx * fx + fz * fz).sqrt()).max(0.0);
            let ripple = (x as f32 * 0.35).sin() * 0.5 + (z as f32 * 0.4).cos() * 0.5;
            let height = (14.0 + mound * 12.0 + ripple) as usize;
            for y in 0..height {
                let material = if y + 1 == height {
                    3
                } else if y + 4 >= height {
                    2
                } else {
                    1
                };
                let index = x | (y << CHUNK_BITS) | (z << (2 * CHUNK_BITS));
                volume[index] = material;
            }
        }
    }
    naive_mesh(&volume)
}
