//! Curated model catalogs for in-chat `/model` switching.
//!
//! Inspired by OpenCode’s `/models` picker UX (provider-scoped lists + free-form
//! ids). Lists are allowlisted defaults for Vaughan’s four providers — not a
//! live models.dev scrape. Users can still type any model id the provider accepts.

use crate::types::{ProviderType, DEFAULT_GEMINI_MODEL, GEMINI_PRO_MODEL};

/// One selectable model in the chat picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    /// Upstream model id (API / `agent.toml` `model` field).
    pub id: &'static str,
    /// Short label shown beside the id in the TUI.
    pub label: &'static str,
}

/// Models offered for `provider` in the `/model` picker.
pub fn models_for_provider(provider: ProviderType) -> &'static [CatalogModel] {
    match provider {
        ProviderType::Ollama => OLLAMA_MODELS,
        ProviderType::Gemini => GEMINI_MODELS,
        ProviderType::OpenAi => OPENAI_MODELS,
        ProviderType::Cursor => CURSOR_MODELS,
    }
}

/// Parse `provider/model` or a bare model id.
///
/// Returns `(optional provider override, model_id)`. A bare id keeps the
/// session provider; a `provider/...` prefix requests that provider.
pub fn parse_model_ref(raw: &str) -> Option<(Option<ProviderType>, String)> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((left, right)) = s.split_once('/') {
        let provider = parse_provider_id(left)?;
        let model = right.trim();
        if model.is_empty() {
            return None;
        }
        return Some((Some(provider), model.to_string()));
    }
    Some((None, s.to_string()))
}

/// Map a short provider id (`ollama`, `gemini`, …) to [`ProviderType`].
pub fn parse_provider_id(raw: &str) -> Option<ProviderType> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ollama" | "local" => Some(ProviderType::Ollama),
        "gemini" | "google" => Some(ProviderType::Gemini),
        "openai" | "openrouter" | "deepseek" => Some(ProviderType::OpenAi),
        "cursor" => Some(ProviderType::Cursor),
        _ => None,
    }
}

/// Stable short id for status chrome (`ollama`, `gemini`, …).
pub fn provider_id(provider: ProviderType) -> &'static str {
    match provider {
        ProviderType::Ollama => "ollama",
        ProviderType::Gemini => "gemini",
        ProviderType::OpenAi => "openai",
        ProviderType::Cursor => "cursor",
    }
}

const OLLAMA_MODELS: &[CatalogModel] = &[
    CatalogModel {
        id: "llama3.2",
        label: "Llama 3.2",
    },
    CatalogModel {
        id: "llama3.1",
        label: "Llama 3.1",
    },
    CatalogModel {
        id: "mistral",
        label: "Mistral",
    },
    CatalogModel {
        id: "qwen2.5",
        label: "Qwen 2.5",
    },
    CatalogModel {
        id: "gemma3",
        label: "Gemma 3",
    },
    CatalogModel {
        id: "deepseek-r1",
        label: "DeepSeek R1",
    },
];

const GEMINI_MODELS: &[CatalogModel] = &[
    CatalogModel {
        id: DEFAULT_GEMINI_MODEL,
        label: "Gemini 3.5 Flash",
    },
    CatalogModel {
        id: GEMINI_PRO_MODEL,
        label: "Gemini 3.5 Pro",
    },
    CatalogModel {
        id: "gemini-3.1-flash-lite",
        label: "Gemini 3.1 Flash-Lite",
    },
];

const OPENAI_MODELS: &[CatalogModel] = &[
    CatalogModel {
        id: "openrouter/free",
        label: "OpenRouter Free (auto)",
    },
    CatalogModel {
        id: "gpt-4o-mini",
        label: "GPT-4o mini",
    },
    CatalogModel {
        id: "gpt-4o",
        label: "GPT-4o",
    },
    CatalogModel {
        id: "gpt-4.1",
        label: "GPT-4.1",
    },
    CatalogModel {
        id: "gpt-4.1-mini",
        label: "GPT-4.1 mini",
    },
    CatalogModel {
        id: "o4-mini",
        label: "o4-mini",
    },
];

const CURSOR_MODELS: &[CatalogModel] = &[
    CatalogModel {
        id: "composer-2",
        label: "Composer 2",
    },
    CatalogModel {
        id: "composer-2-fast",
        label: "Composer 2 Fast",
    },
    CatalogModel {
        id: "gpt-5.4-medium",
        label: "GPT-5.4 Medium",
    },
    CatalogModel {
        id: "claude-4.6-sonnet-medium-thinking",
        label: "Claude 4.6 Sonnet",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_and_qualified() {
        assert_eq!(parse_model_ref("llama3.2"), Some((None, "llama3.2".into())));
        assert_eq!(
            parse_model_ref("gemini/gemini-3.5-pro"),
            Some((Some(ProviderType::Gemini), "gemini-3.5-pro".into()))
        );
        assert!(parse_model_ref("").is_none());
        assert!(parse_model_ref("unknown/foo").is_none());
    }

    #[test]
    fn catalogs_nonempty() {
        for p in [
            ProviderType::Ollama,
            ProviderType::Gemini,
            ProviderType::OpenAi,
            ProviderType::Cursor,
        ] {
            assert!(!models_for_provider(p).is_empty());
        }
    }
}
