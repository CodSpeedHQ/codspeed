#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unused)]

// Use pre-generated bindings instead of generating at build time so that downstream
// users don't need to install `libclang`.
//
// To regenerate bindings, run:
// ```
// ./update-bindings.sh
// ```
include!("bindings.rs");
