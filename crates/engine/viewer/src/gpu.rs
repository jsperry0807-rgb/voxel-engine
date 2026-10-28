//! wgpu initialization, surface configuration and frame presentation.
//!
//! One [`Gpu`] owns the instance's device, queue, window surface, camera
//! uniform and depth buffer. The surface is configured once the window exists
//! and reconfigured whenever the drawable size changes. A zero-sized window
//! (minimized) is left alone: wgpu rejects a zero width or height, and the next
//! [`WindowEvent::Resized`] brings a usable size back.
//!
//! [`Gpu::render`] uploads the camera, acquires the next frame, clears color
//! and depth, draws the depth-test quads and presents. The present waits on the
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

use crate::atlas::AtlasBuilder;
use crate::atlas_texture::AtlasTexture;
use crate::camera::{Camera, CameraUniform};
use crate::depth::DepthTarget;
use crate::error::ViewerError;
use crate::quads::Quads;

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
    /// The two-quad depth-test fixture.
    pub quads: Quads,
    /// The material atlas texture, sampler and bind group.
    pub atlas: AtlasTexture,
}

impl Gpu {
    /// Creates the instance, adapter, device, queue and surface for `window`.
    ///
    /// The display handle comes from the event loop: GLES needs it on Wayland
    /// to create a surface at all.
    pub fn new(window: Arc<Window>, display: OwnedDisplayHandle) -> Result<Self, ViewerError> {
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
        let quads = Quads::new(&device, &camera_layout, config.format);
        // M1.5-12 will bind the baked atlas. Uploading the empty builder here
        // keeps the texture/sampler/bind-group path live in the meantime.
        let atlas = AtlasTexture::new(&device, &queue, &AtlasBuilder::new().build());

        Ok(Self {
            device,
            queue,
            surface,
            config,
            camera_buffer,
            camera_bind_group,
            depth,
            quads,
            atlas,
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
    /// depth, draws the depth-test quads and presents.
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
            self.quads.draw(&mut pass, &self.camera_bind_group);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        Ok(())
    }
}
