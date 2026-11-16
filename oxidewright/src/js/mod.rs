//! JavaScript glue: the bundled injected scripts, Playwright's value
//! serialization format and helpers to build evaluation expressions.

pub(crate) mod serde_impl;
pub mod sources;
pub(crate) mod value;

pub use serde_impl::ValueError;
pub use value::JsValue;

/// Mirrors `normalizeEvaluationExpression` of Playwright's `server/javascript.ts`
/// for string expressions (`isFunction === undefined`): the expression is
/// trimmed and function declarations are wrapped in parentheses so that
/// `eval` returns the function instead of declaring it.
pub(crate) fn normalize_expression(expression: &str) -> String {
    let expression = expression.trim();
    if is_function_declaration(expression) {
        format!("({expression})")
    } else {
        expression.to_string()
    }
}

/// `/^(async)?\s*function(\s|\()/`
fn is_function_declaration(expression: &str) -> bool {
    let rest = expression.strip_prefix("async").unwrap_or(expression);
    let rest = rest.trim_start();
    match rest.strip_prefix("function") {
        Some(after) => after
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || c == '('),
        None => false,
    }
}

/// Builds the source that instantiates a class exported by one of the bundled
/// scripts, the same way Playwright's server does:
/// `(() => { const module = {}; <source>; return new (module.exports.<Class>())(globalThis, <args>); })()`.
pub(crate) fn instantiate_source(source: &str, class: &str, args: &str) -> String {
    format!(
        "(() => {{\nconst module = {{}};\n{source}\nreturn new (module.exports.{class}())(globalThis, {args});\n}})()"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_like_playwright() {
        assert_eq!(normalize_expression(" 1 + 2 "), "1 + 2");
        assert_eq!(
            normalize_expression("function() { return 1 }"),
            "(function() { return 1 })"
        );
        assert_eq!(
            normalize_expression("function foo() {}"),
            "(function foo() {})"
        );
        assert_eq!(
            normalize_expression("async function() {}"),
            "(async function() {})"
        );
        assert_eq!(normalize_expression("() => 1"), "() => 1");
        assert_eq!(normalize_expression("functionName()"), "functionName()");
    }

    #[test]
    fn bundled_scripts_export_their_classes() {
        assert!(sources::INJECTED_SCRIPT.contains("InjectedScript: () => InjectedScript"));
        assert!(sources::UTILITY_SCRIPT.contains("UtilityScript: () => UtilityScript"));
    }
}
