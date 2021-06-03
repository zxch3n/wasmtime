//! stuff having to do with tracing stats... idk yet!

/// A tracing subscriber can check for this target to see spans associated with Wasmtime
/// observability
pub const OBSERV_SPAN: &'static str = "wasmtime_observ";

/// A tracing subscriber can check for this target to see spans associated with Wasmtime
/// observability
pub const OBSERV_FIELD_PREFIX: &'static str = "wasmtime_observ_";

/// Create a tracing span for Wasmtime observability. Provide fields for tracing to associate with
/// the span.
#[macro_export]
macro_rules! observ_span {
    ($($field:tt)*) => {{
        tracing::span!(
            target: wasmtime::tracing::OBSERV_SPAN,
            tracing::Level::TRACE,
            wasmtime::tracing::OBSERV_SPAN,
            $($field)*
        )
    }};
}

#[macro_export]
/// XXX
macro_rules! observ_counter {
    ($k:ident = $field:tt) => {{
        paste::paste!(
            tracing::event!(
                target: $crate::tracing::OBSERV_SPAN,
                tracing::Level::TRACE,
                [< wasmtime_observ_ $k >] = $field,
            )
        )
    }};
}
