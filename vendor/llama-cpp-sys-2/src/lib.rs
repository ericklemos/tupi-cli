#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(clippy::all)]

// Bindings gerados pelo `build.rs` a partir de `wrapper.h` sobre o llama.cpp
// do submódulo (pin em `LLAMA_CPP_TAG`). O fork não expõe API nova: o contrato
// system-one continua sendo responsabilidade do carregador, não deste crate.
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
