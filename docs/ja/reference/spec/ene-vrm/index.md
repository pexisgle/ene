# `ene-vrm` VRM1.0レンダラー仕様

`ene-vrm` クレートは、VRM 1.0 形式のアバターモデルをディスクからロードし、表情（BlendShape）、目線（LookAt）、揺れもの物理シミュレーション（SpringBone）、VRMA形式のアニメーション再生、および `wgpu` を用いた MToon シェーディングによる描画を行うグラフィックスレンダラーモジュールです。

---

## 1. 主要データ構造

### `VrmModel` (公開 / 構造体)
ロードされたアバターの全アセットとボーン情報を保持する最上位オブジェクト。
*   `meshes: Vec<VrmMesh>`: 描画対象のポリゴンメッシュ群。
*   `textures: Vec<VrmTexture>`: テクスチャ画像データ群。
*   `skeleton: Skeleton`: Humanoid 規格ボーンの参照リストと初期トランスフォーム。
*   `humanoid: HumanoidBoneRegistry`: Canonical 規格（55ボーン）から glTF ノードインデックスへのマッピング。
*   `look_at: LookAtProperties`: 目線追従パラメータ。
*   `spring_bones: SpringBoneProperties`: 揺れもの（髪・服等）のジョイント・コリダー定義。

---

## 2. コアサブモジュール

### 1. ロード処理 (`loader::load_vrm`)
*   **シグネチャ**: `pub fn load_vrm(data: &[u8]) -> Result<VrmModel, VrmError>`
*   **解説**:
    1.  `.vrm` バイナリ（gltf/glb 構造）をパース。
    2.  `VRMC_vrm` 拡張スキーマから、Humanoid ボーン、表情ターゲット（Morph Targets）、LookAt（視線追従）パラメータをロード。
    3.  `VRMC_springBone` 拡張から揺れもの構造、衝突体（Collider）を読み込み。
    4.  MToon (トゥーンシェーダーマテリアル) のテクスチャバインドおよびパラメータ情報を解決。

### 2. 骨格構造 (`humanoid.rs`)
*   **Humanoid 規格ボーンの正規化**:
    UnityやVRM規格で定義されている55種類のボーン（Hips, Spine, Head, LeftHand 等）の定義 `VrmBone` と、glTF ノード ID の紐付けを管理。アニメーションリターゲットのベースとなります。

### 3. 表情合成・ブレンドシェイプ (`expression.rs`)
*   **表情ブレンドマトリクス**:
    `VRM1` の表情グループ（Joy, Angry, Sorrow, Fun, Blink 等）に対し、どのメッシュのどの Morph Target（モーフターゲット）のウェイト値を何％適用するかを保持。
*   **衝突回避（Expression Override）**:
    目が瞬きしている間（Blink）は、喜び目（Joy）の表情モーフを一部打ち消すなど、ブレンドの排他・上書き制御を行います。

### 4. 視線追従 (`look_at.rs`)
*   カメラやターゲットの座標に向かってアバターの目を向ける制御。
*   **タイプ判定**:
    -   `LookAtType::Bone`: 目（LeftEye/RightEye）のボーン回転により視線移動。
    -   `LookAtType::Expression`: 目のテクスチャUVやモーフターゲット（表情値）のウェイトを増減させて擬似的に視線移動。

### 5. 揺れものシミュレーション (`spring_bone.rs`)
*   髪やスカートなどの物理シミュレーター。
*   **アルゴリズム**: Verlet 積分（Verlet Integration）を用いた位置解決。
*   **コリジョン判定**: ボーンジョイント球と、腕や体幹に設定された球体・カプセル型コリダー（`SpringBoneCollider`）との間で、毎フレーム交差判定と押し戻し処理を実行します。

### 6. アニメーション再生 (`animation.rs` - VRMA)
*   `.vrma` (VRM Animation) 形式ファイルをロードし、`VrmaClip` として保持。
*   **リターゲット機能**:
    アニメーションファイルのボーン比率と、現在ロードされているアバターモデル（`VrmModel`）のボーン長比率の差分を吸収するための、Hips 位置スケール retargeting およびボーン回転補正を適用します。

### 7. 描画処理 (`renderer.rs`)
*   `wgpu` のレンダーパイプライン、深度バッファ、MToon（トゥーンレンダリング）用バインドグループを管理。
*   **MToon特徴**: 陰影の境界のシャープさ（Shading Toony）、ハイライト、リムライト、輪郭線（Outline）の描画を GPU シェーダー（WGSL）にて描画します。
