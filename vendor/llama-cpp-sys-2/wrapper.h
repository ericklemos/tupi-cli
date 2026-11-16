// Cabeçalho lido pelo bindgen. Os includes apontados pelo `build.rs` são
// `llama.cpp/include` e `llama.cpp/ggml/include` do submódulo pinado em
// `LLAMA_CPP_TAG` (v0.8.0); não há cópia local do header do llama.cpp aqui.
#include "llama.h"
#include "ggml.h"
#include "ggml-alloc.h"
#include "ggml-backend.h"
#include "ggml-cpu.h"
