//! Model load helpers (GPU offload + GGUF path validation + optional mtmd).

use super::backend::with_backend;
use super::map_llama_err;
use crate::config::ProactiveAcceleration;
use crate::error::LlmProviderError;
use llama_cpp_2::LlamaBackendDeviceType;
use llama_cpp_2::list_llama_ggml_backend_devices;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::mtmd::{MtmdContext, MtmdContextParams};
use std::ffi::CString;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

/// GPU offload plan derived from [`ProactiveAcceleration`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct GpuOffload {
    pub n_gpu_layers: u32,
    /// Device index into `list_llama_ggml_backend_devices`, if forcing a GPU.
    pub device_index: Option<usize>,
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

/// Resolve layer count and optional device index from config.
pub(crate) fn resolve_gpu_offload(
    acceleration: ProactiveAcceleration,
    gpu_layers: &str,
) -> Result<GpuOffload, LlmProviderError> {
    let layers = parse_gpu_layers(gpu_layers);
    match acceleration {
        ProactiveAcceleration::Cpu => Ok(GpuOffload {
            n_gpu_layers: 0,
            device_index: None,
        }),
        ProactiveAcceleration::Vulkan => {
            let idx = find_device_index(DevicePrefer::Vulkan)?;
            Ok(GpuOffload {
                n_gpu_layers: layers,
                device_index: Some(idx),
            })
        }
        ProactiveAcceleration::Cuda => {
            let idx = find_device_index(DevicePrefer::Cuda)?;
            Ok(GpuOffload {
                n_gpu_layers: layers,
                device_index: Some(idx),
            })
        }
        ProactiveAcceleration::Auto => {
            if let Ok(idx) = find_device_index(DevicePrefer::Vulkan) {
                return Ok(GpuOffload {
                    n_gpu_layers: layers,
                    device_index: Some(idx),
                });
            }
            if let Ok(idx) = find_device_index(DevicePrefer::Cuda) {
                return Ok(GpuOffload {
                    n_gpu_layers: layers,
                    device_index: Some(idx),
                });
            }
            Ok(GpuOffload {
                n_gpu_layers: 0,
                device_index: None,
            })
        }
    }
}

#[derive(Clone, Copy)]
enum DevicePrefer {
    Vulkan,
    Cuda,
}

fn find_device_index(prefer: DevicePrefer) -> Result<usize, LlmProviderError> {
    // Backend must exist before device enumeration is meaningful.
    with_backend(|_| Ok(()))?;
    let devices = list_llama_ggml_backend_devices();
    for (i, dev) in devices.iter().enumerate() {
        let name = dev.name.to_ascii_lowercase();
        let backend = dev.backend.to_ascii_lowercase();
        let is_gpu = matches!(dev.device_type, LlamaBackendDeviceType::Gpu);
        if !is_gpu {
            continue;
        }
        match prefer {
            DevicePrefer::Vulkan => {
                if backend.contains("vulkan") || name.contains("vulkan") {
                    return Ok(i);
                }
            }
            DevicePrefer::Cuda => {
                if backend.contains("cuda") || name.contains("cuda") {
                    return Ok(i);
                }
            }
        }
    }
    let kind = match prefer {
        DevicePrefer::Vulkan => "Vulkan",
        DevicePrefer::Cuda => "CUDA",
    };
    Err(LlmProviderError::LocalLlm(format!(
        "{kind} device not found among ggml backends"
    )))
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
        self.mtmd.as_ref().is_some_and(MtmdContext::support_vision)
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
    }
    if let Some(idx) = offload.device_index {
        params = params
            .with_devices(&[idx])
            .map_err(|e| map_llama_err("invalid GPU device index", e))?;
    }

    tracing::info!(
        component = "LlamaCpp",
        path = %path.display(),
        n_gpu_layers = offload.n_gpu_layers,
        device_index = ?offload.device_index,
        "Loading GGUF"
    );

    let model = LlamaModel::load_from_file(backend, path, &params)
        .map_err(|e| map_llama_err("failed to load GGUF", e))?;

    let mtmd = if let Some(mmproj) = mmproj_path {
        let use_gpu = offload.n_gpu_layers > 0;
        let mtmd_params = MtmdContextParams {
            use_gpu,
            print_timings: false,
            n_threads: 4,
            media_marker: CString::new(llama_cpp_2::mtmd::mtmd_default_marker())
                .map_err(|e| LlmProviderError::LocalLlm(format!("invalid media marker: {e}")))?,
            // Gemma 4 supported budgets: 70, 140, 280, 560, 1120 — keep modest for overlays.
            image_min_tokens: 70,
            image_max_tokens: 280,
        };
        tracing::info!(
            component = "LlamaCpp",
            path = %mmproj.display(),
            use_gpu,
            "Loading mmproj for vision"
        );
        match MtmdContext::init_from_file(mmproj.to_string_lossy().as_ref(), &model, &mtmd_params) {
            Ok(ctx) => {
                if ctx.support_vision() {
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
