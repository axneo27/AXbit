// indirect_args without normal.

struct Instance { // without normal
    model_matrix: mat4x4<f32>,
    color: vec4<f32>,
    id: vec4<u32>,
};

@group(0) @binding(0) var<storage, read> all_instances: array<Instance>;
@group(0) @binding(1) var<storage, read> visibility_flags: array<u32>;
@group(0) @binding(2) var<storage, read_write> visible_indices: array<u32>;
@group(0) @binding(3) var<storage, read_write> indirect_args: array<u32>; 

struct DrawParams {
    // NOTE: this value is used as index-count when rendering indexed meshes.
    vertex_count_per_mesh: u32,
    _padding: u32,
    _padding2: u32,
    _padding3: u32,
};
@group(0) @binding(4) var<uniform> draw_params: DrawParams;

// global atomic counter stored in a small storage binding (binding 5)
@group(0) @binding(5) var<storage, read_write> visible_count: atomic<u32>;

// First pass: each invocation appends its visible index using an atomicAdd into
// `visible_indices`. This is safe across workgroups because `visible_count` is
// a storage atomic. We cannot reliably finalise the indirect args from the
// same invocation set (no global barrier), so a second small dispatch will
// finalise the indirect args (see `finalize_indirect`).

@compute @workgroup_size(64)
fn generate_indirect(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.x;
    if (idx >= arrayLength(&visibility_flags)) { return; }

    if (visibility_flags[idx] == 1u) {
        let visible_idx = atomicAdd(&visible_count, 1u);
        visible_indices[visible_idx] = idx;
    }
}

// Second pass: single-thread finaliser. It runs after the append phase and
// writes the single DrawIndexedIndirectArgs (index_count, instance_count,
// first_index, first_instance) into `indirect_args` and resets the counter.
@compute @workgroup_size(1)
fn finalize_indirect(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // single-thread finaliser: compact `visible_indices` in ascending order
    if (global_id.x != 0u) { return; }

    var pos: u32 = 0u;
    let n = arrayLength(&visibility_flags);
    var i: u32 = 0u;
    loop {
        if (i >= n) { break; }
        if (visibility_flags[i] == 1u) {
            visible_indices[pos] = i;
            pos = pos + 1u;
        }
        i = i + 1u;
    }

    // write indirect args (index_count, instance_count, first_index, first_instance)
    indirect_args[0] = draw_params.vertex_count_per_mesh;
    indirect_args[1] = pos;
    indirect_args[2] = 0u;
    indirect_args[3] = 0u;

    // reset counter for next frame (append-phase may still run, keep counter consistent)
    atomicStore(&visible_count, 0u);
}