//! Library entry point for the Arth compiler.
//!
//! This exposes the `compiler` module so that other crates (such as
//! a TypeScript front-end) can produce `HirFile` values and reuse
//! the existing lowering pipeline.

#![allow(dead_code)]
// Suppress common clippy warnings in compiler code
// Many of these patterns are intentional for clarity in complex compiler logic
#![allow(clippy::collapsible_if)]
#![allow(clippy::type_complexity)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::manual_strip)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::len_zero)]
#![allow(clippy::needless_return)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::iter_kv_map)]
#![allow(clippy::match_ref_pats)]
#![allow(clippy::get_first)]
#![allow(clippy::clone_on_copy)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::explicit_counter_loop)]
#![allow(clippy::unwrap_or_default)]
#![allow(unused_imports)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::len_without_is_empty)]
#![allow(clippy::redundant_pattern_matching)]
#![allow(clippy::redundant_field_names)]
#![allow(clippy::only_used_in_recursion)]
#![allow(clippy::match_like_matches_macro)]
#![allow(clippy::useless_conversion)]
#![allow(clippy::option_as_ref_deref)]
#![allow(clippy::enum_variant_names)]
#![allow(clippy::unnecessary_get_then_check)]
#![allow(clippy::for_kv_map)]
#![allow(clippy::needless_range_loop)]
#![allow(unused_variables)]

pub mod compiler;
