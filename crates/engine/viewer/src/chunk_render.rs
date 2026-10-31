//! Draw pipeline for one chunk mesh.
//!
//! The pipeline consumes the packed 8-byte vertices from [`ChunkMesh`] and
//! the M2-27 chunk shader (AO + block light). Camera is group 0, atlas group 1
//! and the M2/M3 chunk data (light, probes, world-voxel origin) group 2.

use wgpu::{
    BindGroup, BindGroupLayout, BlendState, Buffer, ColorTargetState, ColorWrites, CompareFunction,
    DepthStencilState, Device, FragmentState, IndexFormat, MultisampleState,
    PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, RenderPass, RenderPipeline,
    RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, StencilState, TextureFormat,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

use voxel_core::content_constants::LIGHT_CELLS_PER_EDGE;

use crate::chunk_mesh::ChunkMesh;
use crate::depth::DEPTH_FORMAT;
use crate::mesh_pool::MeshAllocation;

const CHUNK_WGSL: &str = include_str!("shaders/chunk.wgsl");

/// The packed 8 B production vertex layout.
pub const VERTEX_ATTRIBUTES: [VertexAttribute; 2] = [
    VertexAttribute {
        format: VertexFormat::Uint8x4,
        offset: 0,
        shader_location: 0,
    },
    VertexAttribute {
        format: VertexFormat::Uint16x2,
        offset: 4,
        shader_location: 1,
    },
];

/// The vertex buffer layout for [`ChunkPipeline`].
pub const VERTEX_LAYOUT: VertexBufferLayout<'static> = VertexBufferLayout {
    array_stride: 8,
    step_mode: VertexStepMode::Vertex,
    attributes: &VERTEX_ATTRIBUTES,
};

const _: () = assert!(
    VERTEX_LAYOUT.array_stride == voxel_mesh::PackedVertex::SIZE as u64,
    "the chunk pipeline stride must match PackedVertex"
);

/// The M1.5 naive 16 B layout, kept for the screenshot example and its
/// historical artifacts.
pub const NAIVE_VERTEX_ATTRIBUTES: [VertexAttribute; 3] = [
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

/// The naive 16 B vertex buffer layout.
pub const NAIVE_VERTEX_LAYOUT: VertexBufferLayout<'static> = VertexBufferLayout {
    array_stride: 16,
    step_mode: VertexStepMode::Vertex,
    attributes: &NAIVE_VERTEX_ATTRIBUTES,
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
        data_layout: &BindGroupLayout,
        color_format: TextureFormat,
    ) -> Self {
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("chunk"),
            bind_group_layouts: &[Some(camera_layout), Some(atlas_layout), Some(data_layout)],
            immediate_size: 0,
        });
        // The light-cell edge is a compile-time constant; substitute it so the
        // shader indexes the uploaded light volume correctly at 64³ and 128³.
        let source = CHUNK_WGSL.replace("{{CELLS}}", &format!("{LIGHT_CELLS_PER_EDGE}u"));
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("chunk"),
            source: ShaderSource::Wgsl(source.into()),
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
    ///
    /// This is the one-buffer-per-chunk path used by the M1.5 screenshot
    /// example and the validation fixtures; the streaming viewer draws
    /// [`MeshAllocation`]s out of the M3-02 pool instead.
    pub fn draw(
        &self,
        pass: &mut RenderPass<'_>,
        chunk: &ChunkMesh,
        camera: &BindGroup,
        atlas: &BindGroup,
        data: &BindGroup,
    ) {
        debug_assert_eq!(
            chunk.vertex_stride, VERTEX_LAYOUT.array_stride,
            "chunk mesh stride does not match the packed pipeline"
        );
        self.draw_ranges(
            pass,
            &chunk.vertices,
            0,
            &chunk.indices,
            0,
            chunk.index_count,
            camera,
            atlas,
            data,
        );
    }

    /// Draws one mesh out of the M3-02 pool.
    ///
    /// The vertex and index slices share one page buffer; the draw starts each
    /// at the mesh's offset, so no range crosses a page.
    pub fn draw_pooled(
        &self,
        pass: &mut RenderPass<'_>,
        mesh: &MeshAllocation,
        camera: &BindGroup,
        atlas: &BindGroup,
        data: &BindGroup,
    ) {
        debug_assert_eq!(
            u64::from(mesh.format().vertex_stride()),
            VERTEX_LAYOUT.array_stride,
            "pooled mesh stride does not match the packed pipeline"
        );
        let Some(indices) = &mesh.indices else {
            debug_assert!(false, "the chunk pipeline needs an index stream");
            return;
        };
        self.draw_ranges(
            pass,
            &mesh.vertices.buffer,
            mesh.vertices.offset,
            &indices.buffer,
            indices.offset,
            mesh.index_count(),
            camera,
            atlas,
            data,
        );
    }

    /// Binds the shared state and draws one vertex/index range.
    #[allow(clippy::too_many_arguments)]
    fn draw_ranges(
        &self,
        pass: &mut RenderPass<'_>,
        vertices: &Buffer,
        vertex_offset: u64,
        indices: &Buffer,
        index_offset: u64,
        index_count: u32,
        camera: &BindGroup,
        atlas: &BindGroup,
        data: &BindGroup,
    ) {
        // An empty range draws nothing without a validation error; both the
        // pooled and the per-chunk install paths keep empty meshes out.
        debug_assert!(index_count > 0, "a drawn mesh has at least one index");
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, atlas, &[]);
        pass.set_bind_group(2, data, &[]);
        pass.set_vertex_buffer(0, vertices.slice(vertex_offset..));
        pass.set_index_buffer(indices.slice(index_offset..), IndexFormat::Uint16);
        pass.draw_indexed(0..index_count, 0, 0..1);
    }
}
