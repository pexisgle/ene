# `ene-vrm` VRM 1.0 Character Renderer Specifications

The `ene-vrm` crate loads VRM 1.0 mascot models, evaluates blendshape expressions, handles look-at tracking, simulates spring bone physics, retargets VRMA animations, and renders models using `wgpu` with MToon shading.

---

## 1. Struct Definition & Main Model Methods (`model.rs`)

### `VrmModel` (Public / Struct)
The top-level mascot asset holder containing geometries, skeleton hierarchies, material links, and physics definitions.

#### `new`
*   **Signature**:
    ```rust
    pub const fn new(
        meshes: Vec<VrmMesh>,
        skeleton: Skeleton,
        aabb_min: [f32; 3],
        aabb_max: [f32; 3],
        humanoid: HumanoidBoneRegistry,
        look_at: Option<LookAtProperties>,
        spring_bones: Option<SpringBoneProperties>,
        expression_overrides: Option<Vec<ExpressionDefinition>>,
    ) -> Self
    ```
*   **Description**: Constructs a `VrmModel` asset container.

#### `joint_count`
*   **Signature**: `pub const fn joint_count(&self) -> usize`
*   **Description**: Returns the total number of joints defined in the skeletal hierarchy.

#### `compute_world_transforms`
*   **Signature**: `pub fn compute_world_transforms(&mut self)`
*   **Description**: Traverses the node hierarchy using depth-first search to calculate absolute world transformation matrices (`glam::Mat4`) from relative local transforms.

#### `update_skin_palette`
*   **Signature**: `pub fn update_skin_palette(&mut self, frame: &VrmaFrame, look_at: Option<&LookAtBoneOutput>) -> Vec<Mat4>`
*   **Description**: Updates model bone transforms using VRMA keyframes and eye/head look-at target orientations. Returns the skin joint palette matrix buffer.

#### `rebuild_skin_palette`
*   **Signature**: `pub fn rebuild_skin_palette(&mut self, hips_translation: Option<Vec3>) -> Vec<Mat4>`
*   **Description**: Rebuilds the skin palette using the rest pose, optionally applying translation offsets to the root hips bone.

---

## 2. Loader & GLB Decoders (`loader.rs`)

#### `load_vrm`
*   **Signature**: `pub fn load_vrm(path: impl AsRef<Path>, device: &wgpu::Device, queue: &wgpu::Queue) -> VrmResult<VrmModel>`
*   **Process**:
    1.  Parses the `.vrm` binary GLB payload.
    2.  Extracts expressions, humanoid bone nodes, and gaze configurations from the `VRMC_vrm` glTF extension.
    3.  Extracts spring chains and colliders from the `VRMC_springBone` extension.
    4.  Binds MToon material values and texture mappings.
    5.  Initializes rendering resources and returns the loaded model.

#### `load_all_meshes`
*   **Signature**: `fn load_all_meshes(gltf: &gltf::Gltf, device: &wgpu::Device, queue: &wgpu::Queue, mtoon_materials: &[Option<mtoon::MToonMaterial>], primitive_joint_remap: &[Vec<Vec<u32>>]) -> VrmResult<LoadAllMeshesResult>`
*   **Description**: Decodes glTF meshes, creates vertex and index GPU buffers, and maps material inputs.

#### `load_primitive_morph_targets`
*   **Signature**: `fn load_primitive_morph_targets(primitive: &gltf::Primitive, gltf: &gltf::Gltf, expected_vertex_count: usize, mesh_idx: usize, prim_idx: usize, scale: f32) -> Option<Vec<crate::expression::MorphTarget>>`
*   **Description**: Decodes morph target vertex offset buffers (positions and normals) for blendshapes.

#### `load_merged_skeleton_and_remaps`
*   **Signature**: `fn load_merged_skeleton_and_remaps(gltf: &gltf::Gltf) -> (Skeleton, Vec<Vec<Vec<u32>>>)`
*   **Description**: Extracts rest bones and builds joint indices mapping sub-mesh vertex indices to global bones.

---

## 3. Blendshapes & Expressions (`expression.rs` & `expression_override.rs`)

#### `ExpressionLayer::set_expression`
*   **Signature**: `pub fn set_expression(&mut self, name: &ExpressionName, weight: f32) -> bool`
*   **Description**: Updates the weight of an expression, clamping it between `0.0` and `1.0`.

#### `ExpressionLayer::apply_weights`
*   **Signature**: `pub fn apply_weights(&mut self, incoming: &BTreeMap<ExpressionName, f32>)`
*   **Description**: Batch updates active expression weights.

#### `apply_overrides`
*   **Signature**: `pub fn apply_overrides(weights: &mut BTreeMap<ExpressionName, f32>, defs: &[ExpressionDefinition])`
*   **Description**: Applies expression override rules (e.g., dampening mouth movements or gaze tracking weights when a blink is active).

---

## 4. Gaze Look-At Tracking (`look_at.rs`)

#### `LookAtRangeMap::apply`
*   **Signature**: `pub fn apply(&self, input_degrees: f32) -> f32`
*   **Description**: Maps input angles to target joint rotations or expression weights.

#### `LookAtModel::evaluate`
*   **Signature**: `pub fn evaluate(&self, head_world: Vec3, target_world: Vec3, head_rest_rotation: Quat) -> LookAtOutput`
*   **Description**: Calculates look-at output values (eye bone yaw/pitch or expression weights) based on the world positions of the head and target.

---

## 5. Spring Bone Physics Simulator (`spring_bone.rs`)

#### `SpringBoneSimulator::step`
*   ```rust
    pub fn step(
        &mut self,
        dt: f32,
        props: &SpringBoneProperties,
        node_world_positions: &HashMap<usize, Vec3>,
        node_world_rotations: &HashMap<usize, Quat>,
        node_parent_world_rotations: &HashMap<usize, Quat>,
        collider_world_positions: &HashMap<usize, Vec3>,
        collider_world_rotations: &HashMap<usize, Quat>,
    ) -> HashMap<usize, Quat>
    ```
*   **Process**:
    1.  Computes gravity, inertia, and wind forces on bone joints.
    2.  Uses Verlet integration to update joint positions.
    3.  Resolves collisions with sphere and capsule colliders.
    4.  Converts the final joint positions back to local rotations relative to parent bones.

---

## 6. VRMA Retargeting Player (`animation.rs`)

#### `retarget_rotation`
*   ```rust
    pub fn retarget_rotation(
        src_pose: Quat,
        src_rest_local: Quat,
        src_rest_global: Quat,
        dst_rest_local: Quat,
        dst_rest_global: Quat,
    ) -> Quat
    ```
*   **Description**: Translates animation rotations from the VRMA file coordinate system to the model's bones, compensating for rest pose differences.

#### `retarget_hips_translation`
*   ```rust
    pub fn retarget_hips_translation(
        src_pose: Vec3,
        src_rest_local: Vec3,
        src_rest_global_y: f32,
        dst_rest_local: Vec3,
        dst_rest_global_y: f32,
    ) -> Vec3
    ```
*   **Description**: Scales translation offsets applied to the hips bone by the height ratio between the animation file and the target model.

---

## 7. WGPU Rendering & Post Processing (`renderer.rs` & `post_process.rs`)

#### `VrmRenderer::render`
*   ```rust
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        model: &VrmModel,
        camera: &OrthographicCamera,
        model_uniform: &ModelUniform,
        transparent: bool,
    )
    ```
*   **Description**: Encodes pass commands to render model primitives using MToon shaders.

#### `PostProcessor::render`
*   ```rust
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        dst: &wgpu::TextureView,
    )
    ```
*   **Description**: Applies post-processing shaders (such as SMAA antialiasing or color grading) to intermediate texture buffers.
