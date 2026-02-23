use oxc_ast::ast::Program;
use serde::Serialize;
fn assert_serialize<T: Serialize>() {}
fn main() { assert_serialize::<Program<'_>>(); }
