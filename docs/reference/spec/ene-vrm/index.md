# `ene-vrm` VRM 1.0 Character Renderer Specifications

The `ene-vrm` crate loads VRM 1.0 mascot models, evaluates blendshape expressions, handles look-at tracking, simulates spring bone physics, retargets VRMA animations, and renders models using `wgpu` with MToon shading.

---

## 1. Core Data Structures

### `VrmModel` (Public / Struct)
The top-level mascot asset holder containing:
*   `meshes: Vec<VrmMesh>`: Mesh primitive geometries.
*   `textures: Vec<VrmTexture>`: Loaded image textures.
*   `skeleton: Skeleton`: Rest transforms and bone hierarchies.
*   `humanoid: HumanoidBoneRegistry`: Maps the 55 canonical bones to glTF nodes.
*   `look_at: LookAtProperties`: Settings for gaze tracking.
*   `spring_bones: SpringBoneProperties`: Physics springs and colliders.

---

## 2. Key Submodules

### 1. Model Loading (`loader::load_vrm`)
*   **Signature**: `pub fn load_vrm(data: &[u8]) -> Result<VrmModel, VrmError>`
*   **Process**:
    1.  Parses the `.vrm` binary GLB payload.
    2.  Extracts expressions, humanoid bone nodes, and gaze configurations from the `VRMC_vrm` glTF extension.
    3.  Extracts spring chains and colliders from the `VRMC_springBone` extension.
    4.  Binds MToon material values and texture mappings.

### 2. Humanoid Bone Registry (`humanoid.rs`)
*   Ensures that animators target canonical bone IDs (such as `Hips` or `Head`) rather than arbitrary glTF nodes. The `HumanoidBoneRegistry` resolves structural layouts for animation retargeting.

### 3. Blendshape Expressions (`expression.rs`)
*   **Weights Blend**: Maps abstract expressions (like `Joy` or `Blink`) to vertex morph target weights across meshes.
*   **Expression Overrides (`expression_override.rs`)**:
    Prevents morph conflicts (e.g. scaling down a `Joy` expression morph when a procedural `Blink` takes place).

### 4. Gaze Tracking (`look_at.rs`)
*   Rotates head and eye joints towards a 3D coordinate.
*   **Gaze Type**:
    -   `LookAtType::Bone`: Evaluates rotation offsets on the left/right eye bone nodes.
    -   `LookAtType::Expression`: Shifts texture UV maps or blends expression targets to simulate eye movement.

### 5. Spring Bone Simulator (`spring_bone.rs`)
*   Calculates hair, tail, and clothing physics.
*   **Integration**: Employs Verlet integration to compute point forces.
*   **Collision Detection**: Resolves intersections between joints and capsule/sphere colliders (`SpringBoneCollider`), pushing joints back outside the collision volumes.

### 6. VRMA Player (`animation.rs`)
*   Loads VRM Animation clip data (`.vrma`).
*   **Retargeting**: Performs hips scale translations and retargets bone rotations from the VRMA file coordinates to the loaded model bones.

### 7. WGPU Rendering Pipeline (`renderer.rs`)
*   Orchestrates render passes and outline shaders using WGSL code.
*   **MToon shading**: Draws shading boundaries (Toony settings), specular highlights, rim lights, and outline borders on GPU pipelines.
