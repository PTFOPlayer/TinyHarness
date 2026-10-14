//! OpenRouter-style model variant suffixes.
//!
//! Hosted gateways expose the same underlying model under several variant
//! suffixes, e.g. `deepseek/deepseek-chat-v3-0324:free`. Those suffixed
//! names don't appear verbatim in the gateway's `/v1/models` list (which
//! only contains base IDs), so membership checks must compare the base ID
//! instead of the full string.
//!
//! Known variant suffixes: `:floor`, `:batch`, `:free`.

/// Model variant suffixes that must be ignored when checking whether a
/// model is available on a gateway. Ordering is irrelevant; all suffixes
/// are mutually exclusive in practice, but [`strip_variant_suffixes`]
/// strips any combination if one is ever nested.
pub const VARIANT_SUFFIXES: [&str; 3] = [":free", ":floor", ":batch"];

/// Strip any trailing OpenRouter variant suffix (`:free`, `:floor`,
/// `:batch`) from a model name.
///
/// Returns the base name, or `None` when nothing was stripped (i.e. the
/// name had no recognized suffix) — callers can then fall back to an
/// exact match.
///
/// ```ignore
/// strip_variant_suffixes("deepseek/deepseek-chat:free"); // Some("deepseek/deepseek-chat")
/// strip_variant_suffixes("gpt-4o");                      // None
/// ```
pub fn strip_variant_suffixes(name: &str) -> Option<&str> {
    // Iterate so nested suffixes (e.g. ":free:floor") are fully removed.
    let mut stripped = name;
    let mut stripped_any = false;
    while let Some(base) = VARIANT_SUFFIXES
        .iter()
        .find_map(|suffix| stripped.strip_suffix(suffix))
    {
        stripped = base;
        stripped_any = true;
    }
    (stripped_any && !stripped.is_empty()).then_some(stripped)
}

/// Check whether `name` is available in `models`, ignoring variant
/// suffixes.
///
/// Matching rules:
/// 1. Exact match on the full name, or
/// 2. `name` has a variant suffix (e.g. `:free`) and its base id matches a
///    listed model.
///
/// Examples:
/// * `is_variant_available("a:free", &["a"])` → `true`
/// * `is_variant_available("a:free", &["a:free"])` → `true` (exact)
/// * `is_variant_available("a", &["a:free"])` → `false`
///   (suffixes are only ignored on the *requested* name)
/// * `is_variant_available("a:batch", &["b"])` → `false`
pub fn is_variant_available(name: &str, models: &[String]) -> bool {
    if models.iter().any(|m| m == name) {
        return true;
    }
    strip_variant_suffixes(name).is_some_and(|base| models.iter().any(|m| m == base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_free_suffix() {
        assert_eq!(
            strip_variant_suffixes("deepseek/deepseek-chat:free"),
            Some("deepseek/deepseek-chat")
        );
    }

    #[test]
    fn strip_floor_suffix() {
        assert_eq!(
            strip_variant_suffixes("deepseek/deepseek-chat:floor"),
            Some("deepseek/deepseek-chat")
        );
    }

    #[test]
    fn strip_batch_suffix() {
        assert_eq!(
            strip_variant_suffixes("deepseek/deepseek-chat:batch"),
            Some("deepseek/deepseek-chat")
        );
    }

    #[test]
    fn strip_no_suffix_returns_none() {
        assert_eq!(strip_variant_suffixes("gpt-4o"), None);
        assert_eq!(strip_variant_suffixes("deepseek/deepseek-chat"), None);
    }

    #[test]
    fn strip_colon_without_known_suffix_returns_none() {
        // Ollama-style tags (e.g. "qwen2.5:7b") must NOT be treated as variants.
        assert_eq!(strip_variant_suffixes("qwen2.5:7b"), None);
    }

    #[test]
    fn strip_all_of_nested_suffixes() {
        assert_eq!(
            strip_variant_suffixes("deepseek/deepseek-chat:free:floor"),
            Some("deepseek/deepseek-chat")
        );
    }

    #[test]
    fn strip_empty_base_returns_none() {
        // ":free" alone would strip to an empty name — treat as no suffix.
        assert_eq!(strip_variant_suffixes(":free"), None);
    }

    #[test]
    fn availability_exact_match() {
        let models = vec!["gpt-4o".to_string(), "claude-3.5-sonnet".to_string()];
        assert!(is_variant_available("gpt-4o", &models));
        assert!(!is_variant_available("gpt-4o-mini", &models));
    }

    #[test]
    fn availability_suffix_matches_base_id() {
        let models = vec!["deepseek/deepseek-chat-v3-0324".to_string()];
        assert!(is_variant_available(
            "deepseek/deepseek-chat-v3-0324:free",
            &models
        ));
        assert!(is_variant_available(
            "deepseek/deepseek-chat-v3-0324:floor",
            &models
        ));
        assert!(is_variant_available(
            "deepseek/deepseek-chat-v3-0324:batch",
            &models
        ));
    }

    #[test]
    fn availability_exact_suffixed_entry_matches() {
        // If the gateway actually lists the suffixed id, it matches exactly.
        let models = vec!["deepseek/deepseek-chat:free".to_string()];
        assert!(is_variant_available("deepseek/deepseek-chat:free", &models));
    }

    #[test]
    fn availability_suffix_only_ignored_on_requested_name() {
        // Requesting the base id must not match a suffixed listing (the
        // gateway would reject `deepseek/deepseek-chat` if it only exposes
        // the `:free` variant under that spelling).
        let models = vec!["deepseek/deepseek-chat:free".to_string()];
        assert!(!is_variant_available("deepseek/deepseek-chat", &models));
    }

    #[test]
    fn availability_unrelated_model_is_not_available() {
        let models = vec!["some/other-model".to_string()];
        assert!(!is_variant_available(
            "deepseek/deepseek-chat:free",
            &models
        ));
        assert!(!is_variant_available(
            "deepseek/deepseek-chat:batch",
            &models
        ));
    }

    #[test]
    fn availability_empty_model_list() {
        assert!(!is_variant_available("deepseek/deepseek-chat:free", &[]));
    }

    #[test]
    fn ollama_style_tags_are_not_variants() {
        // Ollama tags like "qwen2.5:7b" share the ":" separator but are not
        // OpenRouter variants — they must only match exactly.
        let models = vec!["qwen2.5".to_string()];
        assert!(!is_variant_available("qwen2.5:7b", &models));

        let models = vec!["qwen2.5:7b".to_string()];
        assert!(is_variant_available("qwen2.5:7b", &models));
    }

    #[test]
    fn suffix_word_is_part_of_larger_segment() {
        // "model:freedom" ends with ":free" as a substring? No — strip_suffix
        // only removes exact trailing ":free"; ":freedom" doesn't end with it.
        let models = vec!["model".to_string()];
        assert!(!is_variant_available("model:freedom", &models));
    }
}
