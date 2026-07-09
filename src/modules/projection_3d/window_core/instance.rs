use cgmath::{Matrix, SquareMatrix};

#[derive(Clone)]
pub struct Instance {
    pub position: cgmath::Vector3<f32>,
    pub rotation: cgmath::Quaternion<f32>,
    pub scale: cgmath::Vector3<f32>,
    pub color: [f32; 3],
    pub id: Option<i32>,
    // Identifies the ID namespace after the picking pass resolves an
    // instance index back to this CPU-side object.
    pub is_sbdb: bool,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRaw {
    model: [[f32; 4]; 4],
    normal: [[f32; 3]; 3],
    color: [f32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRawWithoutNormal {
    model: [[f32; 4]; 4],
    color: [f32; 4], 
    id: [u32; 4],
}

impl InstanceRaw {
    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 4]>() as wgpu::BufferAddress,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 12]>() as wgpu::BufferAddress,
                    shader_location: 8,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 16]>() as wgpu::BufferAddress,
                    shader_location: 9,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 19]>() as wgpu::BufferAddress,
                    shader_location: 10,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 22]>() as wgpu::BufferAddress,
                    shader_location: 11,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 25]>() as wgpu::BufferAddress,
                    shader_location: 12,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}

impl InstanceRawWithoutNormal {
    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<InstanceRawWithoutNormal>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 4]>() as wgpu::BufferAddress,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 12]>() as wgpu::BufferAddress,
                    shader_location: 8,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 16]>() as wgpu::BufferAddress,
                    shader_location: 9,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 20]>() as wgpu::BufferAddress,
                    shader_location: 10,
                    format: wgpu::VertexFormat::Uint32x4,
                },
            ],
        }
    }
}

impl Instance {             
    pub fn to_raw(&self) -> InstanceRaw {

        let translation = cgmath::Matrix4::from_translation(self.position);
        let rotation = cgmath::Matrix4::from(self.rotation);
        let scale = cgmath::Matrix4::from_nonuniform_scale(self.scale.x, self.scale.y, self.scale.z);

        let model = translation * rotation * scale;

        let model_3x3 = cgmath::Matrix3::new(
            model.x.x, model.x.y, model.x.z,
            model.y.x, model.y.y, model.y.z,
            model.z.x, model.z.y, model.z.z,
        );
        
        let normal_matrix = model_3x3.invert()
            .map(|inv| inv.transpose())
            .unwrap_or(cgmath::Matrix3::identity());  
        
        InstanceRaw {
            model: model.into(),
            normal: normal_matrix.into(),
            color: self.color,
        }
    }

    pub fn to_raw_without_normal(&self) -> InstanceRawWithoutNormal {

        let translation = cgmath::Matrix4::from_translation(self.position);
        let rotation = cgmath::Matrix4::from(self.rotation);
        let scale = cgmath::Matrix4::from_nonuniform_scale(self.scale.x, self.scale.y, self.scale.z);

        let model = translation * rotation * scale;
        
        // mark planetary barycenters with a priority flag in id[1]
        // (keeps storage layout unchanged). Detection: NAIF planet bodies
        // use the `...*100 + 99` convention (199, 299, ..., 999).
        let mut priority_flag: u32 = 0;
        if let Some(id_val) = self.id {
            if id_val >= 199 && id_val <= 999 && id_val % 100 == 99 {
                priority_flag = 1u32;
            }
        }

        // Keep the SBDB type in the existing metadata layout. Picking now
        // encodes array indices for every object and resolves the type on CPU.
        let is_sbdb_flag: u32 = if self.is_sbdb { 1u32 } else { 0u32 };

        InstanceRawWithoutNormal {
            model: model.into(),
            color: [self.color[0], self.color[1], self.color[2], 1.0],
            // id[0] = numeric id, id[1] = priority flag, id[2] = SBDB flag.
            id: [self.id.unwrap_or(0) as u32, priority_flag, is_sbdb_flag, 0],
        }
    }
}
