//! Draw pipeline for one chunk mesh.
//!
//! The pipeline consumes the ADR-010 16-byte vertices from [`ChunkMesh`] and
//! the M1.5-12 chunk shader. Camera group 0 and atlas group 1 match the rest of
//! the viewer's binding convention.

use wgpu::{
    BindGroup, BindGroupLayout, BlendState, ColorTargetState, ColorWrites, CompareFunction,
    DepthStencilState, Device, FragmentState, IndexFormat, MultisampleState,
    PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, RenderPass, RenderPipeline,
    RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, StencilState, TextureFormat,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

use crate::chunk_mesh::ChunkMesh;
use crate::depth::DEPTH_FORMAT;

const CHUNK_WGSL: &str = include_str!("shaders/chunk.wgsl");

/// The ADR-010 16 B vertex layout, shared with offline tools that draw the
/// same vertex stream (the screenshot example).
pub const VERTEX_ATTRIBUTES: [VertexAttribute; 3] = [
    VertexAttribute {
        format: VertexFormat::Uint16x4,
        offset: 0,
        shader_location: 0,
    },
    VertexAttribute {
        format: VertexFormat::Uint8x4,
        offset: 8,
        shader_location: 1,
    },
    VertexAttribute {
        format: VertexFormat::Uint8x4,
        offset: 12,
        shader_location: 2,
    },
];

/// The vertex buffer layout for [`ChunkPipeline`].
pub const VERTEX_LAYOUT: VertexBufferLayout<'static> = VertexBufferLayout {
    array_stride: 16,
    step_mode: VertexStepMode::Vertex,
    attributes: &VERTEX_ATTRIBUTES,
};

/// The chunk render pipeline.
pub struct ChunkPipeline {
    pipeline: RenderPipeline,
}

impl ChunkPipeline {
    /// Builds the chunk pipeline for `color_format`.
    pub fn new(
        device: &Device,
        camera_layout: &BindGroupLayout,
        atlas_layout: &BindGroupLayout,
        color_format: TextureFormat,
    ) -> Self {
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("chunk"),
            bind_group_layouts: &[Some(camera_layout), Some(atlas_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("chunk"),
            source: ShaderSource::Wgsl(CHUNK_WGSL.into()),
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("chunk"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(VERTEX_LAYOUT)],
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(ColorTargetState {
                    format: color_format,
                    blend: Some(BlendState::REPLACE),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::Less),
                stencil: StencilState::default(),
                bias: Default::default(),
            }),
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline }
    }

    /// Draws `chunk` with the camera and atlas bind groups.
    pub fn draw(
        &self,
        pass: &mut RenderPass<'_>,
        chunk: &ChunkMesh,
        camera: &BindGroup,
        atlas: &BindGroup,
    ) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, atlas, &[]);
        pass.set_vertex_buffer(0, chunk.vertices.slice(..));
        pass.set_index_buffer(chunk.indices.slice(..), IndexFormat::Uint16);
        pass.draw_indexed(0..chunk.index_count, 0, 0..1);
    }
}
