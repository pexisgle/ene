# `ene-vrm` VRM 1.0 キャラクターレンダラー仕様

`ene-vrm` クレートは、VRM 1.0 キャラクターモデルのロード、表情ブレンドシェイプの補間評価、視線追跡（Look-At）、揺れもの物理シミュレーション（Spring Bone）、VRMA アニメーションの読み込みとリターゲット、および `wgpu` による MToon シェーディング描画を管理します。

---

## 1. コアモデルデータ構造と操作メソッド (`model.rs`)

### `VrmModel` (パブリック / 構造体)
キャラクターのメッシュ、テクスチャ、ボーン骨格、表情定義、および物理設定情報を集約して保持するルートコンポーネントです。

#### `new`
*   **定義**:
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
*   **説明**: `VrmModel` キャラクターアセットモデルコンテナを作成します。

#### `joint_count`
*   **シグネチャ**: `pub const fn joint_count(&self) -> usize`
*   **説明**: キャラクターのボーン骨格内に存在するボーン（ジョイント）の総数を返します。

#### `compute_world_transforms`
*   **シグネチャ**: `pub fn compute_world_transforms(&mut self)`
*   **説明**: 親子階層構造に沿って深さ優先探索（DFS）を行い、各ジョイントのローカル変化行列を、レンダリング時に使用する絶対ワールド変換行列（`glam::Mat4`）へと累積計算します。

#### `update_skin_palette`
*   **シグネチャ**: `pub fn update_skin_palette(&mut self, frame: &VrmaFrame, look_at: Option<&LookAtBoneOutput>) -> Vec<Mat4>`
*   **説明**: 現在の VRMA キーフレームポーズ、および視線追跡（Look-At）による関節回転量を合成してボーン座標を更新し、GPU のスキンメッシュシェーダーに入力するジョイント変換行列配列（パレット）を返します。

#### `rebuild_skin_palette`
*   **シグネチャ**: `pub fn rebuild_skin_palette(&mut self, hips_translation: Option<Vec3>) -> Vec<Mat4>`
*   **説明**: レストポーズ（初期姿勢）に基づき、必要に応じてヒップ位置の初期補正オフセットを適用してスキンパレット行列配列を再構築します。

---

## 2. VRM モデルロード処理 (`loader.rs`)

#### `load_vrm`
*   **シグネチャ**: `pub fn load_vrm(path: impl AsRef<Path>, device: &wgpu::Device, queue: &wgpu::Queue) -> VrmResult<VrmModel>`
*   **プロセス**:
    1.  指定された `.vrm` バイナリ GLB ファイルデータを解析します。
    2.  `VRMC_vrm` glTF 拡張ブロックから、表情定義、55種類の標準 Humanoid ジョイントノードマッピング、および視線追跡パラメータをロードします。
    3.  `VRMC_springBone` 拡張ブロックから、揺れものボーンチェーンの定義とコライダーの配置設定を抽出します。
    4.  MToon シェーダー用マテリアル定数とテクスチャリソースを解決して GPU にバインドします。
    5.  すべてを結合した `VrmModel` モデルオブジェクトを構築します。

#### `load_all_meshes`
*   **シグネチャ**: `fn load_all_meshes(gltf: &gltf::Gltf, device: &wgpu::Device, queue: &wgpu::Queue, mtoon_materials: &[Option<mtoon::MToonMaterial>], primitive_joint_remap: &[Vec<Vec<u32>>]) -> VrmResult<LoadAllMeshesResult>`
*   **説明**: メッシュジオメトリの頂点バッファおよびインデックスバッファを作成し、対応する MToon マテリアルパラメータを割り当てます。

#### `load_primitive_morph_targets`
*   **シグネチャ**: `fn load_primitive_morph_targets(primitive: &gltf::Primitive, gltf: &gltf::Gltf, expected_vertex_count: usize, mesh_idx: usize, prim_idx: usize, scale: f32) -> Option<Vec<crate::expression::MorphTarget>>`
*   **説明**: 表情変更時に頂点位置を動的にブレンド補間するために、頂点モーフターゲットバッファの差分（座標、法線ベクトル）データをデコードします。

#### `load_merged_skeleton_and_remaps`
*   **シグネチャ**: `fn load_merged_skeleton_and_remaps(gltf: &gltf::Gltf) -> (Skeleton, Vec<Vec<Vec<u32>>>)`
*   **説明**: ボーンの初期変形情報を読み込み、頂点ウェイトジョイントインデックスをグローバルなボーン ID にマッピング再配置します。

---

## 3. 表情ブレンドシェイプと競合排除 (`expression.rs` & `expression_override.rs`)

#### `ExpressionLayer::set_expression`
*   **シグネチャ**: `pub fn set_expression(&mut self, name: &ExpressionName, weight: f32) -> bool`
*   **説明**: 表情名（`Joy`、`Blink` など）に対するブレンドシェイプの重み（Weight）を `[0.0, 1.0]` の範囲にクランプしてセットします。

#### `ExpressionLayer::apply_weights`
*   **シグネチャ**: `pub fn apply_weights(&mut self, incoming: &BTreeMap<ExpressionName, f32>)`
*   **説明**: 複数の表情ブレンドシェイプパラメータを一括で更新適用します。

#### `apply_overrides`
*   **シグネチャ**: `pub fn apply_overrides(weights: &mut BTreeMap<ExpressionName, f32>, defs: &[ExpressionDefinition])`
*   **説明**: 表情競合ルールを処理します。たとえば `Blink`（まばたき）が 100% 適用されている間は、目の変形が破綻しないように `Joy` などによる他の目元モーフウェイトを一時的に減衰・ブロック（Block / Blend）します。

---

## 4. 視線追跡 Look-At 処理 (`look_at.rs`)

#### `LookAtRangeMap::apply`
*   **シグネチャ**: `pub fn apply(&self, input_degrees: f32) -> f32`
*   **説明**: 入力された目標物に対する角度パラメータ（度）を、指定されたカーブ特性に沿って実際のボーン回転角度または表情モーフウェイトへとマッピング計算します。

#### `LookAtModel::evaluate`
*   **シグネチャ**: `pub fn evaluate(&self, head_world: Vec3, target_world: Vec3, head_rest_rotation: Quat) -> LookAtOutput`
*   **説明**: 頭の現在位置および追跡目標物（Target）のワールド座標から、首および目のジョイントに適用すべき目標回転量を算出します。

---

## 5. 揺れもの Spring Bone 物理シミュレーション (`spring_bone.rs`)

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
*   **プロセス**:
    1.  髪の毛や服などのボーン関節の先端点に対して、重力、慣性力、風力などの外部フォースを算出します。
    2.  Verlet（ベルレ）積分を実行して、新たなワールド座標を仮決定します。
    3.  球体およびカプセルコライダー（`SpringBoneCollider`）との境界交差を検証し、めり込んでいる場合は衝突領域の外側へ点を押し出します。
    4.  計算された関節先端のワールド位置から、ジョイントの親に対する相対的な局所回転量（`Quat`）に逆算出・マッピングして返します。

---

## 6. VRM アニメーション (VRMA) プレイヤー仕様 (`animation.rs`)

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
*   **説明**: 汎用の VRMA アニメーションファイル内の骨格構造と、現在ロードされている VRM mascot の関節レストポーズ形状の違いを相殺するように、回転角を数学的に retarget（適合変換）します。

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
*   **説明**: モデルのスケール身長比率（アニメーションファイルと対象モデルの頭頂高比など）を算出し、ルートボーン（Hips）の平行移動移動量をスケール補正して適用します。

---

## 7. WGPU レンダリングおよびポストプロセス処理 (`renderer.rs` & `post_process.rs`)

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
*   **説明**: キャラクターの各不透明・半透明プリミティブを、MToon シェーダーを使用して描画パイプラインに投入し、Outline や陰影境界を GPU レンダリングします。

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
*   **説明**: 描画された中間テクスチャバッファに対し、アンチエイリアシング（SMAA）や色変換フィルタなどのポストプロセス加工を適用し、最終的なスワップチェーンビューへ転送・出力します。
