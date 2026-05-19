pub fn format_token_count(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ContextWindowSize {
    Small8K,
    Medium32K,
    Large128K,
    Huge256K,
    Custom(u32),
}

impl ContextWindowSize {
    pub fn tokens(&self) -> u32 {
        match self {
            ContextWindowSize::Small8K => 8192,
            ContextWindowSize::Medium32K => 32768,
            ContextWindowSize::Large128K => 131072,
            ContextWindowSize::Huge256K => 262144,
            ContextWindowSize::Custom(n) => *n,
        }
    }

    pub fn default_size() -> Self {
        ContextWindowSize::Small8K
    }

    pub fn usage_percentage(&self, used_tokens: u32) -> f64 {
        let max = self.tokens() as f64;
        let used = used_tokens as f64;
        (used / max * 100.0).min(100.0)
    }
}

pub const WARNING_THRESHOLD_PERCENT: f64 = 70.0;
pub const CRITICAL_THRESHOLD_PERCENT: f64 = 90.0;

pub fn check_context_warning(
    used_tokens: u32,
    context_size: ContextWindowSize,
) -> Option<ContextWarning> {
    let percentage = context_size.usage_percentage(used_tokens);

    if percentage >= CRITICAL_THRESHOLD_PERCENT {
        Some(ContextWarning::Critical(percentage))
    } else if percentage >= WARNING_THRESHOLD_PERCENT {
        Some(ContextWarning::Warning(percentage))
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ContextWarning {
    Warning(f64),
    Critical(f64),
}

impl ContextWarning {
    pub fn percentage(&self) -> f64 {
        match self {
            ContextWarning::Warning(p) | ContextWarning::Critical(p) => *p,
        }
    }

    pub fn is_critical(&self) -> bool {
        matches!(self, ContextWarning::Critical(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_token_count() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(100), "100");
        assert_eq!(format_token_count(1000), "1.0K");
        assert_eq!(format_token_count(1500), "1.5K");
        assert_eq!(format_token_count(1000000), "1.0M");
        assert_eq!(format_token_count(1500000), "1.5M");
    }

    #[test]
    fn test_context_window_default() {
        // Default is conservative 8K
        assert!(matches!(
            ContextWindowSize::default_size(),
            ContextWindowSize::Small8K
        ));
    }

    #[test]
    fn test_context_window_usage() {
        let ctx = ContextWindowSize::Small8K;
        assert!((ctx.usage_percentage(4096) - 50.0).abs() < 1.0);
        assert!((ctx.usage_percentage(8192) - 100.0).abs() < 1.0);
    }

    #[test]
    fn test_context_warning() {
        let ctx = ContextWindowSize::Small8K;

        // No warning at 50%
        assert!(check_context_warning(4096, ctx).is_none());

        // Warning at 70% (8192 * 0.70 = 5734.4)
        let warning = check_context_warning(5735, ctx);
        assert!(warning.is_some());
        assert!(!warning.unwrap().is_critical());

        // Critical at 90% (8192 * 0.90 = 7372.8)
        let critical = check_context_warning(7373, ctx);
        assert!(critical.is_some());
        assert!(critical.unwrap().is_critical());
    }
}
