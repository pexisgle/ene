//! Model load helpers (GPU offload + GGUF path validation + optional mtmd).

use super::backend::with_backend;
use super::map_llama_err;
use ene_ai::ResourceClass;
use ene_ai::config::ProactiveAcceleration;
use ene_ai::error::LlmProviderError;
use llama_cpp_4::context::LlamaContext;
use llama_cpp_4::context::params::LlamaContextParams;
use llama_cpp_4::llama_backend::LlamaBackend;
use llama_cpp_4::model::LlamaModel;
use llama_cpp_4::model::params::LlamaModelParams;
use llama_cpp_4::mtmd::{MtmdContext, MtmdContextParams};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

/// GPU offload plan derived from [`ProactiveAcceleration`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct GpuOffload {
    pub n_gpu_layers: u32,
    /// When true, pin offload to ggml main GPU index 0 (built Cargo backend).
    pub use_main_gpu: bool,
}

/// Parameters for loading a GGUF into llama.cpp.
#[derive(Debug, Clone)]
pub(crate) struct LoadSpec {
    pub model_path: PathBuf,
    pub mmproj_path: Option<PathBuf>,
    pub acceleration: ProactiveAcceleration,
    pub gpu_layers: String,
    pub context_size: u32,
}

impl LoadSpec {
    pub(crate) fn validate_model_path(path: &str) -> Result<PathBuf, LlmProviderError> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(LlmProviderError::LocalLlm(
                "local GGUF model_path is empty".to_string(),
            ));
        }
        let path = PathBuf::from(trimmed);
        if !path.is_file() {
            return Err(LlmProviderError::LocalLlm(format!(
                "model file not found: {}",
                path.display()
            )));
        }
        Ok(path)
    }
}

/// Resolve layer count and whether to use `with_main_gpu(0)`.
///
/// llama-cpp-4 has no pre-load device enumeration; GPU choice is the backend
/// compiled via Cargo features (`vulkan` / `cuda`) plus main GPU index 0.
pub(crate) fn resolve_gpu_offload(
    acceleration: ProactiveAcceleration,
    gpu_layers: &str,
) -> Result<GpuOffload, LlmProviderError> {
    let layers = parse_gpu_layers(gpu_layers);
    match acceleration {
        ProactiveAcceleration::Cpu => Ok(GpuOffload {
            n_gpu_layers: 0,
            use_main_gpu: false,
        }),
        ProactiveAcceleration::Vulkan => {
            if cfg!(feature = "vulkan") {
                Ok(GpuOffload {
                    n_gpu_layers: layers,
                    use_main_gpu: true,
                })
            } else {
                Err(LlmProviderError::LocalLlm(
                    "Vulkan acceleration requested but ene-ai-local was built without the `vulkan` feature"
                        .to_string(),
                ))
            }
        }
        ProactiveAcceleration::Cuda => {
            if cfg!(feature = "cuda") {
                Ok(GpuOffload {
                    n_gpu_layers: layers,
                    use_main_gpu: true,
                })
            } else {
                Err(LlmProviderError::LocalLlm(
                    "CUDA acceleration requested but ene-ai-local was built without the `cuda` feature"
                        .to_string(),
                ))
            }
        }
        ProactiveAcceleration::Auto => {
            if cfg!(feature = "vulkan") || cfg!(feature = "cuda") {
                Ok(GpuOffload {
                    n_gpu_layers: layers,
                    use_main_gpu: true,
                })
            } else {
                Ok(GpuOffload {
                    n_gpu_layers: 0,
                    use_main_gpu: false,
                })
            }
        }
    }
}

/// The [`ResourceClass`] a model loaded with `spec` contends on.
///
/// `with_main_gpu: true` means `load_with_backend` below called
/// `.with_main_gpu(0)` — the same physical device index every other
/// GPU-offloaded local engine in this crate uses, so this always returns
/// device `0` (there is no per-model device selection today; see this
/// crate's docs on `ResourceRegistry` for why sharing that one class across
/// independently-constructed engines is exactly the point).
pub(crate) fn resource_class_for(spec: &LoadSpec) -> Result<ResourceClass, LlmProviderError> {
    let offload = resolve_gpu_offload(spec.acceleration, &spec.gpu_layers)?;
    Ok(if offload.use_main_gpu {
        ResourceClass::Gpu { device: 0 }
    } else {
        ResourceClass::Cpu
    })
}

fn parse_gpu_layers(raw: &str) -> u32 {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        999
    } else {
        trimmed.parse().unwrap_or(999)
    }
}

/// Owned loaded model + optional multimodal projector context.
pub(crate) struct LoadedModel {
    pub model: LlamaModel,
    pub context_size: NonZeroU32,
    pub mtmd: Option<MtmdContext>,
    /// File stem of the model path, used for model family detection.
    pub model_name: String,
}

impl LoadedModel {
    pub(crate) fn load(spec: &LoadSpec) -> Result<Self, LlmProviderError> {
        let offload = resolve_gpu_offload(spec.acceleration, &spec.gpu_layers)?;
        with_backend(|backend| {
            load_with_backend(
                backend,
                &spec.model_path,
                spec.mmproj_path.as_deref(),
                offload,
                spec.context_size,
            )
        })
    }

    #[must_use]
    pub(crate) fn supports_vision(&self) -> bool {
        self.mtmd.as_ref().is_some_and(MtmdContext::supports_vision)
    }

    /// Builds a [`LlamaContext`] for this model with its borrow unsafely
    /// widened from `&self`'s real lifetime to `'static`.
    ///
    /// # Why this exists
    ///
    /// `LlamaContext<'a>` borrows `&'a LlamaModel`. This crate's worker
    /// models (`LlamaChatModel`, `LlamaEmbedModel`) want to cache one
    /// `LlamaContext` alongside the `LoadedModel` it was built from —
    /// reused across jobs instead of rebuilt per request — which is a
    /// self-referential struct and not expressible with a checked lifetime.
    ///
    /// # Safety
    ///
    /// The caller must:
    /// - box (or otherwise heap-pin) the `LoadedModel` this is called on, so
    ///   its address stays fixed regardless of how the *caller's own*
    ///   struct is moved — the returned context's `'static` claim is a lie
    ///   that is only true for as long as `self`'s address does not change;
    /// - drop the returned context before `self` is dropped or replaced
    ///   (e.g. put the `Option<LlamaContext<'static>>` field ahead of the
    ///   `Box<LoadedModel>` field, or clear it explicitly in a manual `Drop`
    ///   impl before the model can go away).
    ///
    /// Both worker models in this crate satisfy this: `self` is a boxed
    /// field never moved or replaced while a cached context exists, and each
    /// clears its `ctx` field before ever touching `loaded` again.
    pub(crate) unsafe fn new_context_unbounded(
        &self,
        backend: &LlamaBackend,
        params: LlamaContextParams,
    ) -> Result<LlamaContext<'static>, LlmProviderError> {
        let ctx = self
            .model
            .new_context(backend, params)
            .map_err(|e| map_llama_err("failed to create llama context", e))?;
        // SAFETY: widening `LlamaContext<'_>` (tied to `&self.model`, i.e.
        // `&'_ self`) to `'static`. Sound exactly when this function's own
        // safety contract (see doc comment above) is upheld by the caller.
        Ok(unsafe { std::mem::transmute::<LlamaContext<'_>, LlamaContext<'static>>(ctx) })
    }
}

fn load_with_backend(
    backend: &LlamaBackend,
    path: &Path,
    mmproj_path: Option<&Path>,
    offload: GpuOffload,
    context_size: u32,
) -> Result<LoadedModel, LlmProviderError> {
    let mut params = LlamaModelParams::default();
    if offload.n_gpu_layers > 0 {
        params = params.with_n_gpu_layers(offload.n_gpu_layers);
        if offload.use_main_gpu {
            params = params.with_main_gpu(0);
        }
    }

    tracing::info!(
        component = "LlamaCpp",
        path = %path.display(),
        n_gpu_layers = offload.n_gpu_layers,
        use_main_gpu = offload.use_main_gpu,
        "Loading GGUF"
    );

    let model = LlamaModel::load_from_file(backend, path, &params)
        .map_err(|e| map_llama_err("failed to load GGUF", e))?;

    let mtmd = if let Some(mmproj) = mmproj_path {
        let use_gpu = offload.n_gpu_layers > 0;
        let mtmd_params = MtmdContextParams::default()
            .use_gpu(use_gpu)
            .print_timings(false)
            .n_threads(4)
            // Gemma 4 supported budgets: 70, 140, 280, 560, 1120 — keep modest for overlays.
            .image_min_tokens(70)
            .image_max_tokens(280);
        tracing::info!(
            component = "LlamaCpp",
            path = %mmproj.display(),
            use_gpu,
            "Loading mmproj for vision"
        );
        match MtmdContext::init_from_file(mmproj, &model, mtmd_params) {
            Ok(ctx) => {
                if ctx.supports_vision() {
                    Some(ctx)
                } else {
                    tracing::warn!(
                        component = "LlamaCpp",
                        "mmproj loaded but vision is not supported"
                    );
                    None
                }
            }
            Err(e) => {
                tracing::warn!(
                    component = "LlamaCpp",
                    error = %e,
                    "failed to load mmproj; continuing text-only"
                );
                None
            }
        }
    } else {
        None
    };

    let ctx = NonZeroU32::new(context_size.max(256))
        .ok_or_else(|| LlmProviderError::LocalLlm("context_size must be non-zero".to_string()))?;

    let model_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(LoadedModel {
        model,
        context_size: ctx,
        mtmd,
        model_name,
    })
}
