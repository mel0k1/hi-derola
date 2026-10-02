#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelInfo {
    pub window: u64,
    pub input: f64,
    pub output: f64,
}

const FALLBACK: ModelInfo = ModelInfo {
    window: 128_000,
    input: 0.0,
    output: 0.0,
};

const CATALOG: &[(&str, u64, f64, f64)] = &[
    // openai, usd per 1m tokens (input, output)
    ("gpt-5", 400_000, 1.25, 10.0),
    ("gpt-5-mini", 400_000, 0.25, 2.0),
    ("gpt-5-nano", 400_000, 0.05, 0.4),
    ("gpt-4.1", 1_000_000, 2.0, 8.0),
    ("gpt-4.1-mini", 1_000_000, 0.4, 1.6),
    ("gpt-4.1-nano", 1_000_000, 0.1, 0.4),
    ("gpt-4o", 128_000, 2.5, 10.0),
    ("gpt-4o-mini", 128_000, 0.15, 0.6),
    ("gpt-4-turbo", 128_000, 10.0, 30.0),
    ("gpt-4", 8_000, 30.0, 60.0),
    ("gpt-3.5-turbo", 16_000, 0.5, 1.5),
    ("o1-pro", 200_000, 150.0, 600.0),
    ("o1", 200_000, 15.0, 60.0),
    ("o1-mini", 128_000, 1.1, 4.4),
    ("o3-pro", 200_000, 20.0, 80.0),
    ("o3", 200_000, 2.0, 8.0),
    ("o3-mini", 200_000, 1.1, 4.4),
    ("o4-mini", 200_000, 1.1, 4.4),
    // anthropic
    ("claude-opus-4-5", 200_000, 5.0, 25.0),
    ("claude-opus-4", 200_000, 15.0, 75.0),
    ("claude-sonnet-4-5", 200_000, 3.0, 15.0),
    ("claude-sonnet-4", 200_000, 3.0, 15.0),
    ("claude-haiku-4-5", 200_000, 1.0, 5.0),
    ("claude-haiku-4", 200_000, 1.0, 5.0),
    ("claude-3-7-sonnet", 200_000, 3.0, 15.0),
    ("claude-3-5-sonnet", 200_000, 3.0, 15.0),
    ("claude-3-5-haiku", 200_000, 0.8, 4.0),
    ("claude-3-opus", 200_000, 15.0, 75.0),
    ("claude-3-sonnet", 200_000, 3.0, 15.0),
    ("claude-3-haiku", 200_000, 0.25, 1.25),
    // google
    ("gemini-2.5-pro", 1_048_576, 1.25, 10.0),
    ("gemini-2.5-flash", 1_048_576, 0.3, 2.5),
    ("gemini-2.5-flash-lite", 1_048_576, 0.1, 0.4),
    ("gemini-2.0-flash", 1_048_576, 0.1, 0.4),
    ("gemini-2.0-flash-lite", 1_048_576, 0.075, 0.3),
    ("gemini-1.5-pro", 2_000_000, 1.25, 5.0),
    ("gemini-1.5-flash", 1_048_576, 0.075, 0.3),
    // deepseek
    ("deepseek-chat", 128_000, 0.27, 1.1),
    ("deepseek-reasoner", 128_000, 0.55, 2.19),
    ("deepseek-v3", 128_000, 0.27, 1.1),
    ("deepseek-r1", 128_000, 0.55, 2.19),
    // xai
    ("grok-4", 256_000, 3.0, 15.0),
    ("grok-4-fast", 2_000_000, 0.2, 0.5),
    ("grok-3", 131_000, 3.0, 15.0),
    ("grok-3-mini", 131_000, 0.3, 0.5),
    // moonshot
    ("kimi-k2", 256_000, 0.6, 2.5),
    ("kimi-latest", 128_000, 0.6, 2.5),
    // zhipu
    ("glm-4.6", 200_000, 0.6, 2.2),
    ("glm-4.5", 128_000, 0.6, 2.2),
    ("glm-4.5-air", 128_000, 0.2, 1.1),
    // alibaba
    ("qwen3-coder", 256_000, 0.3, 1.2),
    ("qwen3-max", 256_000, 1.2, 6.0),
    ("qwen3", 128_000, 0.2, 0.8),
    ("qwq", 128_000, 0.15, 0.6),
];

pub fn lookup(model: &str) -> ModelInfo {
    let m = model.trim().to_lowercase();
    let m = m.rsplit('/').next().unwrap_or(&m);
    let m = m.trim();
    if m.is_empty() {
        return FALLBACK;
    }
    let mut best: Option<&(&str, u64, f64, f64)> = None;
    for entry in CATALOG {
        let name = entry.0;
        let hit = m == name
            || (m.len() > name.len()
                && m.starts_with(name)
                && matches!(m.as_bytes()[name.len()], b'-' | b'.' | b'_' | b'@'));
        if hit {
            let better = best.map(|b| name.len() > b.0.len()).unwrap_or(true);
            if better {
                best = Some(entry);
            }
        }
    }
    match best {
        Some((_, window, input, output)) => ModelInfo {
            window: *window,
            input: *input,
            output: *output,
        },
        None => FALLBACK,
    }
}

pub fn cost(model: &str, input: u64, output: u64) -> f64 {
    let mi = lookup(model);
    input as f64 / 1e6 * mi.input + output as f64 / 1e6 * mi.output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_models() {
        let gpt5 = lookup("gpt-5");
        assert_eq!(gpt5.window, 400_000);
        assert!((gpt5.input - 1.25).abs() < 1e-9);

        let dated = lookup("gpt-4o-2024-11-20");
        assert_eq!(dated.window, 128_000);
        assert!((dated.input - 2.5).abs() < 1e-9);

        let mini = lookup("gpt-4o-mini");
        assert!(mini.input < dated.input, "longest prefix must win");

        let claude = lookup("anthropic/claude-sonnet-4.5");
        assert_eq!(claude.window, 200_000);
        assert!((claude.output - 15.0).abs() < 1e-9);

        let dotted = lookup("claude-sonnet-4-20250514");
        assert_eq!(dotted.window, 200_000);
        assert!((dotted.input - 3.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_models_fall_back() {
        let mi = lookup("my-local-llm");
        assert_eq!(mi.window, 128_000);
        assert_eq!(mi.input, 0.0);
        assert_eq!(lookup(""), mi);
        assert_eq!(lookup("  "), mi);
    }

    #[test]
    fn cost_math() {
        let c = cost("gpt-4o", 1_000_000, 100_000);
        assert!((c - (2.5 + 1.0)).abs() < 1e-9);
        assert_eq!(cost("unknown-thing", 10_000_000, 0), 0.0);
    }
}
