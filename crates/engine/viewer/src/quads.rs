//! Two overlapping debug quads that make the depth test visible.
//!
//! The near (red) quad is drawn first and the far (blue) quad second. Both
//! write depth, so the far quad is rejected where the near one already covers
//! it; without the depth test the blue quad would paint over the red one.
//! This is the M1.5-05 fixture and is replaced by the chunk renderer.

use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupLayout, BlendState, Buffer, BufferUsages, ColorTargetState, ColorWrites,
    CompareFunction, DepthStencilState, Device, FragmentState, MultisampleState, PipelineLayout,
    PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, RenderPass, RenderPipeline,
    RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, StencilState, TextureFormat,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
    util::DeviceExt,
};

use crate::depth::DEPTH_FORMAT;

const QUAD_WGSL: &str = include_str!("shaders/quads.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    color: [f32; 3],
}

impl Vertex {
    const ATTRIBUTES: [VertexAttribute; 2] = [
        VertexAttribute {
            format: VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        VertexAttribute {
            format: VertexFormat::Float32x3,
            offset: 12,
            shader_location: 1,
        },
    ];

    const LAYOUT: VertexBufferLayout<'static> = VertexBufferLayout {
        array_stride: std::mem::size_of::<Self>() as u64,
        step_mode: VertexStepMode::Vertex,
        attributes: &Self::ATTRIBUTES,
    };
}

/// The debug quad pipeline and its vertex buffer.
pub struct Quads {
    pipeline: RenderPipeline,
    vertices: Buffer,
    vertex_count: u32,
}

impl Quads {
    /// Builds the pipeline for `color_format` and uploads the two quads.
    pub fn new(
        device: &Device,
        camera_layout: &BindGroupLayout,
        color_format: TextureFormat,
    ) -> Self {
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("debug quads"),
            bind_group_layouts: &[Some(camera_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("debug quads"),
            source: ShaderSource::Wgsl(QUAD_WGSL.into()),
        });
        let pipeline = create_pipeline(device, &pipeline_layout, &shader, color_format);

        let vertices = vertices();
        let vertex_count = vertices.len() as u32;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("debug quad vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: BufferUsages::VERTEX,
        });

        Self {
            pipeline,
            vertices,
            vertex_count,
        }
    }

    /// Draws both quads into `pass`.
    pub fn draw(&self, pass: &mut RenderPass<'_>, camera: &BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

fn create_pipeline(
    device: &Device,
    layout: &PipelineLayout,
    shader: &wgpu::ShaderModule,
    color_format: TextureFormat,
) -> RenderPipeline {
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("debug quads"),
        layout: Some(layout),
        vertex: VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(Vertex::LAYOUT)],
        },
        fragment: Some(FragmentState {
            module: shader,
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
            cull_mode: None,
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
    })
}

/// Near red quad first, far blue quad second, six vertices each.
fn vertices() -> Vec<Vertex> {
    let mut vertices = Vec::with_capacity(12);
    push_quad(&mut vertices, [-0.4, 0.3, 2.0], 1.2, [0.9, 0.15, 0.1]);
    push_quad(&mut vertices, [0.2, -0.1, 0.0], 1.6, [0.1, 0.3, 0.9]);
    vertices
}

fn push_quad(vertices: &mut Vec<Vertex>, center: [f32; 3], half: f32, color: [f32; 3]) {
    let [cx, cy, cz] = center;
    let corners = [
        [cx - half, cy - half, cz],
        [cx + half, cy - half, cz],
        [cx + half, cy + half, cz],
        [cx - half, cy - half, cz],
        [cx + half, cy + half, cz],
        [cx - half, cy + half, cz],
    ];
    vertices.extend(corners.map(|position| Vertex { position, color }));
}
