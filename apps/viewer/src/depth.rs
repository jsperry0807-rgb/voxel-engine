//! Depth buffer for the main render pass.
//!
//! The texture matches the swap chain size and is recreated whenever the
//! surface is reconfigured. Depth is cleared to `1.0` (the far plane) every
//! frame.

use wgpu::{
    Device, Extent3d, Texture, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    TextureView, TextureViewDescriptor,
};

/// Depth format used by the main pass and its pipelines.
pub const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

/// A depth texture and its view.
pub struct DepthTarget {
    /// Kept alive for as long as the view is used.
    _texture: Texture,
    /// The attachment view passed to the render pass.
    pub view: TextureView,
}

impl DepthTarget {
    /// Allocates a depth texture for a `width × height` drawable.
    pub fn new(device: &Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("depth"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        Self {
            _texture: texture,
            view,
        }
    }
}
