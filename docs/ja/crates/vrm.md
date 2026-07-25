# `ene-vrm` — API リファレンス

> **クレート**: `ene-vrm` | **役割**: `ene-desktop` 向け VRM 1.0 3D モデルローダー & wgpu レンダラー

`ene-vrm` は `gltf` および `wgpu` をベースにした VRM 1.0 ファイル専用のローダーおよびレンダラーです。

---

## アーキテクチャ上の保証
- `ene-mind`, `ene-runtime`, `ene-store` に**一切依存・インポートしません**。
- メッシュ、テクスチャ、ボーン変換行列、ブレンドシェイプウェイトを受け取る純粋なグラフィックレンダリングエンジンです。

---

## 主要 API

```rust
pub struct VrmModel { /* ... */ }

impl VrmModel {
    /// バイナリ GLTF (.vrm) バッファから VRM 1.0 モデルを読み込みます。
    pub fn load_from_bytes(bytes: &[u8]) -> Result<Self, VrmError>;

    /// ボーン変換行列およびブレンドシェイプ表情ウェイトを更新します。
    pub fn update_pose(&mut self, pose: &VrmPose);

    /// 指定された wgpu RenderPass を使用してモデルを描画します。
    pub fn render<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>);
}
```

---

## 関連ドキュメント
- [音声とアバターの概念](../concepts/voice-and-avatar.md)
- [Desktop アプリケーションガイド](../apps/desktop.md)
