pub struct ModelInfo {
    pub provider: &'static str,
    pub id: &'static str,
    pub context_window: u32,
    pub max_tokens: u32,
    pub cost_in: f64,
    pub cost_out: f64,
}

const ROWS: &[(&str, &str, u32, u32, f64, f64)] = &[
    (
        "anthropic",
        "claude-fable-5",
        1000000,
        128000,
        10.0000,
        50.0000,
    ),
    (
        "anthropic",
        "claude-haiku-4-5",
        200000,
        64000,
        0.0000,
        0.0000,
    ),
    (
        "anthropic",
        "claude-haiku-4-5-20251001",
        200000,
        64000,
        0.0000,
        0.0000,
    ),
    (
        "anthropic",
        "claude-mythos-5",
        1000000,
        128000,
        10.0000,
        50.0000,
    ),
    (
        "anthropic",
        "claude-opus-4-6",
        200000,
        64000,
        0.0000,
        0.0000,
    ),
    (
        "anthropic",
        "claude-opus-4-7",
        200000,
        64000,
        0.0000,
        0.0000,
    ),
    (
        "anthropic",
        "claude-opus-4-8",
        1048576,
        128000,
        0.0000,
        0.0000,
    ),
    (
        "anthropic",
        "claude-opus-5",
        1000000,
        128000,
        0.0000,
        0.0000,
    ),
    (
        "anthropic",
        "claude-sonnet-4-6",
        200000,
        64000,
        0.0000,
        0.0000,
    ),
    (
        "anthropic",
        "claude-sonnet-5",
        1000000,
        128000,
        0.0000,
        0.0000,
    ),
    ("byteplus", "ark-code-latest", 256000, 4096, 0.0001, 0.0002),
    ("byteplus", "doubao-seed-code", 256000, 4096, 0.0001, 0.0002),
    ("byteplus", "glm-4-7-251222", 200000, 4096, 0.0001, 0.0002),
    ("byteplus", "glm-4.7", 200000, 4096, 0.0001, 0.0002),
    (
        "byteplus",
        "kimi-k2-5-260127",
        256000,
        32768,
        0.6000,
        2.5000,
    ),
    (
        "byteplus",
        "kimi-k2-thinking",
        256000,
        32768,
        0.6000,
        2.5000,
    ),
    ("byteplus", "kimi-k2.5", 256000, 32768, 0.6000, 2.5000),
    ("byteplus", "seed-1-8-251228", 256000, 4096, 0.0001, 0.0002),
    ("cohere", "command-a-03-2025", 256000, 8000, 2.5000, 10.0000),
    ("meta", "muse-spark-1.1", 1048576, 131072, 0.0000, 0.0000),
    ("mistral", "codestral-latest", 256000, 4096, 0.3000, 0.9000),
    (
        "mistral",
        "devstral-medium-latest",
        262144,
        32768,
        0.4000,
        2.0000,
    ),
    ("mistral", "magistral-small", 128000, 40000, 0.5000, 1.5000),
    (
        "mistral",
        "mistral-large-latest",
        262144,
        16384,
        0.5000,
        1.5000,
    ),
    (
        "mistral",
        "mistral-medium-2508",
        262144,
        8192,
        0.4000,
        2.0000,
    ),
    (
        "mistral",
        "mistral-medium-3-5",
        262144,
        8192,
        1.5000,
        7.5000,
    ),
    (
        "mistral",
        "mistral-small-latest",
        128000,
        16384,
        0.1000,
        0.3000,
    ),
    (
        "mistral",
        "pixtral-large-latest",
        128000,
        32768,
        2.0000,
        6.0000,
    ),
    (
        "novita",
        "deepseek/deepseek-r1-0528",
        163840,
        65536,
        0.0000,
        0.0000,
    ),
    (
        "novita",
        "deepseek/deepseek-v3-0324",
        163840,
        65536,
        0.0000,
        0.0000,
    ),
    (
        "novita",
        "minimax/minimax-m2.7",
        1000000,
        65536,
        0.0000,
        0.0000,
    ),
    (
        "novita",
        "moonshotai/kimi-k2.5",
        262144,
        65536,
        0.0000,
        0.0000,
    ),
    (
        "novita",
        "qwen/qwen3-235b-a22b-fp8",
        262144,
        65536,
        0.0000,
        0.0000,
    ),
    ("novita", "zai-org/glm-5", 202752, 65536, 0.0000, 0.0000),
    (
        "nvidia",
        "minimaxai/minimax-m2.7",
        196608,
        8192,
        0.0000,
        0.0000,
    ),
    (
        "nvidia",
        "moonshotai/kimi-k2.5",
        262144,
        8192,
        0.0000,
        0.0000,
    ),
    (
        "nvidia",
        "nvidia/nemotron-3-super-120b-a12b",
        1048576,
        8192,
        0.0000,
        0.0000,
    ),
    (
        "nvidia",
        "nvidia/nemotron-3-ultra-550b-a55b",
        1000000,
        16384,
        0.0000,
        0.0000,
    ),
    ("nvidia", "z-ai/glm-5.1", 202752, 8192, 0.0000, 0.0000),
    ("ollama", "glm-5.1:cloud", 128000, 8192, 0.0000, 0.0000),
    ("ollama", "glm-5.2:cloud", 1000000, 8192, 0.0000, 0.0000),
    ("ollama", "kimi-k2.5:cloud", 128000, 8192, 0.0000, 0.0000),
    ("ollama", "minimax-m2.7:cloud", 128000, 8192, 0.0000, 0.0000),
    (
        "openai",
        "gpt-5.3-chat-latest",
        128000,
        16384,
        1.7500,
        14.0000,
    ),
    ("openai", "gpt-5.3-codex", 400000, 128000, 1.7500, 14.0000),
    ("openai", "gpt-5.4", 272000, 128000, 2.5000, 15.0000),
    ("openai", "gpt-5.4-mini", 400000, 128000, 0.7500, 4.5000),
    ("openai", "gpt-5.4-nano", 400000, 128000, 0.2000, 1.2500),
    ("openai", "gpt-5.4-pro", 1050000, 128000, 30.0000, 180.0000),
    ("openai", "gpt-5.5", 1000000, 128000, 5.0000, 30.0000),
    ("openai", "gpt-5.5-pro", 1000000, 128000, 30.0000, 180.0000),
    ("openai", "gpt-5.6", 1050000, 128000, 5.0000, 30.0000),
    ("openai", "gpt-5.6-luna", 1050000, 128000, 1.0000, 6.0000),
    ("openai", "gpt-5.6-sol", 1050000, 128000, 5.0000, 30.0000),
    ("openai", "gpt-5.6-terra", 1050000, 128000, 2.5000, 15.0000),
    ("openai", "o1", 200000, 100000, 15.0000, 60.0000),
    ("openai", "o1-pro", 200000, 100000, 150.0000, 600.0000),
    ("openai", "o3", 200000, 100000, 2.0000, 8.0000),
    (
        "openai",
        "o3-deep-research",
        200000,
        100000,
        10.0000,
        40.0000,
    ),
    ("openai", "o3-mini", 200000, 100000, 1.1000, 4.4000),
    ("openai", "o3-pro", 200000, 100000, 20.0000, 80.0000),
    ("openai", "o4-mini", 200000, 100000, 1.1000, 4.4000),
    (
        "openai",
        "o4-mini-deep-research",
        200000,
        100000,
        2.0000,
        8.0000,
    ),
    (
        "opencode",
        "claude-opus-4-8",
        200000,
        65536,
        5.0000,
        25.0000,
    ),
    (
        "opencode",
        "deepseek-v4-flash",
        1000000,
        384000,
        0.1400,
        0.2800,
    ),
    (
        "opencode",
        "deepseek-v4-pro",
        1000000,
        384000,
        1.7400,
        3.4800,
    ),
    (
        "opencode",
        "gemini-3.1-pro",
        1048576,
        65536,
        2.0000,
        12.0000,
    ),
    ("opencode", "gpt-5.5", 400000, 128000, 5.0000, 30.0000),
    ("opencode", "minimax-m2.7", 204800, 131072, 0.3000, 1.2000),
    (
        "together",
        "Qwen/Qwen2.5-7B-Instruct-Turbo",
        32768,
        8192,
        0.3000,
        0.3000,
    ),
    (
        "together",
        "deepseek-ai/DeepSeek-V4-Pro",
        512000,
        8192,
        2.1000,
        4.4000,
    ),
    (
        "together",
        "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        131072,
        8192,
        0.8800,
        0.8800,
    ),
    (
        "together",
        "moonshotai/Kimi-K2.6",
        262144,
        32768,
        1.2000,
        4.5000,
    ),
    ("together", "zai-org/GLM-5.1", 202752, 8192, 1.4000, 4.4000),
    (
        "volcengine",
        "ark-code-latest",
        256000,
        4096,
        0.0001,
        0.0002,
    ),
    (
        "volcengine",
        "deepseek-v3-2-251201",
        128000,
        4096,
        0.0001,
        0.0002,
    ),
    (
        "volcengine",
        "doubao-seed-1-8-251228",
        256000,
        4096,
        0.0001,
        0.0002,
    ),
    (
        "volcengine",
        "doubao-seed-code",
        256000,
        4096,
        0.0001,
        0.0002,
    ),
    (
        "volcengine",
        "doubao-seed-code-preview-251028",
        256000,
        4096,
        0.0001,
        0.0002,
    ),
    ("volcengine", "glm-4-7-251222", 200000, 4096, 0.0001, 0.0002),
    ("volcengine", "glm-4.7", 200000, 4096, 0.0001, 0.0002),
    (
        "volcengine",
        "kimi-k2-5-260127",
        256000,
        4096,
        0.0001,
        0.0002,
    ),
    (
        "volcengine",
        "kimi-k2-thinking",
        256000,
        4096,
        0.0001,
        0.0002,
    ),
    ("volcengine", "kimi-k2.5", 256000, 4096, 0.0001, 0.0002),
    ("xiaomi", "mimo-v2-flash", 262144, 8192, 0.0000, 0.0000),
    ("xiaomi", "mimo-v2-omni", 262144, 32000, 0.0000, 0.0000),
    ("xiaomi", "mimo-v2-pro", 1048576, 32000, 0.0000, 0.0000),
    ("xiaomi", "mimo-v2.5", 1048576, 131072, 0.4000, 2.0000),
    ("xiaomi", "mimo-v2.5-pro", 1048576, 131072, 1.0000, 3.0000),
];

const FRIENDLY_NAMES: &[(&str, &str)] = &[
    ("claude-fable-5", "Claude Fable 5"),
    ("claude-haiku-4-5", "Claude Haiku 4.5"),
    ("claude-mythos-5", "Claude Mythos 5"),
    ("claude-opus-4-8", "Claude Opus 4.8"),
    ("claude-opus-5", "Claude Opus 5"),
    ("claude-sonnet-5", "Claude Sonnet 5"),
    ("command-a-03-2025", "Command A"),
    ("deepseek-ai/DeepSeek-V4-Pro", "DeepSeek V4 Pro"),
    ("deepseek/deepseek-r1-0528", "DeepSeek R1"),
    ("deepseek-v4-flash", "DeepSeek V4 Flash"),
    ("deepseek-v4-pro", "DeepSeek V4 Pro"),
    ("gemini-3.1-pro", "Gemini 3.1 Pro"),
    ("glm-5.1:cloud", "GLM 5.1 Cloud"),
    ("glm-5.2:cloud", "GLM 5.2 Cloud"),
    ("gpt-5.4", "GPT-5.4"),
    ("gpt-5.4-mini", "GPT-5.4 Mini"),
    ("gpt-5.4-nano", "GPT-5.4 Nano"),
    ("gpt-5.4-pro", "GPT-5.4 Pro"),
    ("gpt-5.5", "GPT-5.5"),
    ("gpt-5.5-pro", "GPT-5.5 Pro"),
    ("gpt-5.6", "GPT-5.6"),
    ("gpt-5.6-luna", "GPT-5.6 Luna"),
    ("gpt-5.6-sol", "GPT-5.6 Sol"),
    ("gpt-5.6-terra", "GPT-5.6 Terra"),
    ("kimi-k2.5:cloud", "Kimi K2.5 Cloud"),
    ("meta-llama/Llama-3.3-70B-Instruct-Turbo", "Llama 3.3 70B"),
    ("minimax-m2.7:cloud", "MiniMax M2.7 Cloud"),
    ("moonshotai/kimi-k2.5", "Kimi K2.5"),
    ("nvidia/nemotron-3-super-120b-a12b", "Nemotron 3 Super 120B"),
    ("nvidia/nemotron-3-ultra-550b-a55b", "Nemotron 3 Ultra 550B"),
    ("o1", "OpenAI o1"),
    ("o1-pro", "OpenAI o1 Pro"),
    ("o3", "OpenAI o3"),
    ("o3-mini", "OpenAI o3 Mini"),
    ("o3-pro", "OpenAI o3 Pro"),
    ("o4-mini", "OpenAI o4 Mini"),
];

pub fn friendly_name(model: &str) -> Option<&'static str> {
    FRIENDLY_NAMES
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(model))
        .map(|(_, name)| *name)
}

pub fn display_name(model: &str) -> String {
    match friendly_name(model) {
        Some(name) => format!("{name} · {model}"),
        None => model.to_string(),
    }
}

pub fn lookup(provider: &str, model: &str) -> Option<ModelInfo> {
    ROWS.iter()
        .find(|(p, m, ..)| *p == provider && *m == model)
        .or_else(|| ROWS.iter().find(|(_, m, ..)| *m == model))
        .map(
            |&(provider, id, context_window, max_tokens, cost_in, cost_out)| ModelInfo {
                provider,
                id,
                context_window,
                max_tokens,
                cost_in,
                cost_out,
            },
        )
}

pub fn context_window(provider: &str, model: &str) -> Option<u32> {
    lookup(provider, model).map(|i| i.context_window)
}

pub fn known_models(provider: &str) -> Vec<&'static str> {
    ROWS.iter()
        .filter(|(p, ..)| *p == provider)
        .map(|(_, m, ..)| *m)
        .collect()
}

pub fn estimate_cost(provider: &str, model: &str, tokens_in: u64, tokens_out: u64) -> Option<f64> {
    let i = lookup(provider, model)?;
    if i.cost_in == 0.0 && i.cost_out == 0.0 {
        return None;
    }
    Some((tokens_in as f64 * i.cost_in + tokens_out as f64 * i.cost_out) / 1_000_000.0)
}

#[cfg(test)]
pub fn row_count() -> usize {
    ROWS.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_is_not_empty_and_has_no_zero_windows() {
        assert!(row_count() >= 80, "catalog shrank: {}", row_count());
        for (p, m, cw, ..) in ROWS {
            assert!(*cw > 0, "{p}/{m} has a zero context window");
        }
    }

    #[test]
    fn curated_names_keep_the_raw_id_visible() {
        assert_eq!(friendly_name("gpt-5.6-sol"), Some("GPT-5.6 Sol"));
        assert_eq!(display_name("gpt-5.6-sol"), "GPT-5.6 Sol · gpt-5.6-sol");
        assert_eq!(display_name("vendor/new-model"), "vendor/new-model");
    }

    #[test]
    fn opus_five_is_catalogued_and_named() {
        assert_eq!(
            context_window("anthropic", "claude-opus-5"),
            Some(1_000_000)
        );
        assert_eq!(friendly_name("claude-opus-5"), Some("Claude Opus 5"));
        assert!(known_models("anthropic").contains(&"claude-opus-5"));
    }

    #[test]
    fn a_namespaced_model_id_is_found_under_its_provider() {
        let w = context_window("nvidia", "nvidia/nemotron-3-super-120b-a12b")
            .expect("the model benchmarked against a live API must be in the catalog");
        assert_eq!(w, 1_048_576);
    }

    #[test]
    fn an_unknown_model_reports_nothing_rather_than_guessing() {
        assert!(context_window("nvidia", "not-a-real-model").is_none());
    }

    #[test]
    fn cost_is_none_when_the_provider_publishes_no_price() {
        assert!(estimate_cost("nvidia", "nvidia/nemotron-3-super-120b-a12b", 1000, 1000).is_none());
    }

    #[test]
    fn cost_math_is_per_million_tokens() {
        let c = estimate_cost("anthropic", "claude-fable-5", 1_000_000, 0).expect("priced");
        assert!((c - 10.0).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn every_catalog_provider_is_a_known_kind() {
        for (p, ..) in ROWS {
            assert!(
                crate::config::known_kind(p),
                "{p} is in the catalog but not a provider kind, so it can never be selected"
            );
        }
    }
}
