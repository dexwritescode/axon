//! Which kind of server Axon talks to, each with its own quirks on top of the
//! OpenAI API, and how hard a reasoning model on it should think.

use std::str::FromStr;

/// The inference server behind the OpenAI-compatible base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    LmStudio,
    Ollama,
    LlamaCpp,
    Vllm,
    /// Plain OpenAI API, no server-specific extras.
    OpenAi,
}

/// A backend name that isn't one of `Backend::NAMES`.
#[derive(Debug, thiserror::Error)]
#[error("unknown backend {0:?}; expected one of {names}", names = Backend::NAMES.join(", "))]
pub struct UnknownBackend(String);

impl Backend {
    /// The names `from_str` accepts.
    pub const NAMES: [&str; 5] = ["lmstudio", "ollama", "llamacpp", "vllm", "openai"];
}

impl FromStr for Backend {
    type Err = UnknownBackend;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "lmstudio" => Ok(Self::LmStudio),
            "ollama" => Ok(Self::Ollama),
            "llamacpp" => Ok(Self::LlamaCpp),
            "vllm" => Ok(Self::Vllm),
            "openai" => Ok(Self::OpenAi),
            other => Err(UnknownBackend(other.into())),
        }
    }
}

/// How hard a reasoning model thinks before it answers; `None` asks it not to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
}

/// A reasoning effort that isn't one of `ReasoningEffort::NAMES`.
#[derive(Debug, thiserror::Error)]
#[error("unknown reasoning effort {0:?}; expected one of {names}", names = ReasoningEffort::NAMES.join(", "))]
pub struct UnknownReasoningEffort(String);

impl ReasoningEffort {
    /// The names `from_str` accepts.
    pub const NAMES: [&str; 4] = ["none", "low", "medium", "high"];
}

impl FromStr for ReasoningEffort {
    type Err = UnknownReasoningEffort;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "none" => Ok(Self::None),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            other => Err(UnknownReasoningEffort(other.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_parses() {
        for name in Backend::NAMES {
            assert!(name.parse::<Backend>().is_ok(), "{name}");
        }
    }

    #[test]
    fn an_unknown_name_lists_the_known_ones() {
        let err = "lm-studio".parse::<Backend>().unwrap_err().to_string();

        assert_eq!(
            err,
            r#"unknown backend "lm-studio"; expected one of lmstudio, ollama, llamacpp, vllm, openai"#
        );
    }

    #[test]
    fn every_reasoning_effort_parses_and_an_unknown_one_lists_them() {
        for name in ReasoningEffort::NAMES {
            assert!(name.parse::<ReasoningEffort>().is_ok(), "{name}");
        }
        let err = "off".parse::<ReasoningEffort>().unwrap_err().to_string();
        assert_eq!(
            err,
            r#"unknown reasoning effort "off"; expected one of none, low, medium, high"#
        );
    }
}
