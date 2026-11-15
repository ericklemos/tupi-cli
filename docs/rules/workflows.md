# Workflow nativo nasce em `configs/workflows/*.ron`

**Context:** `crates/tupi-tools/build.rs:5-45` varre `configs/workflows`, gera `native_workflows.rs` dentro de `OUT_DIR` e embute o corpo de cada `.ron` com `include_str!`; a tool `workflow` só enumera o que está na const `NATIVE`. Um workflow escrito só em markdown, ou uma edição no arquivo gerado, não aparece em `workflow list` e morre no próximo build.
**Rule:** TODO workflow nativo DEVE ser criado e alterado apenas em `configs/workflows/<slug>.ron`, com o texto integral dentro do campo `body`. NUNCA editar `native_workflows.rs` em `target/` nem esperar que um `.md` fora dessa pasta vire workflow.
**Example (Do):** criar `configs/workflows/<slug>.ron` com `name`, `description` e `body`; recompilar `tupi-tools` (o `rerun-if-changed` já força a regeneração).
**Example (Don't):** copiar o texto do workflow para `target/.../out/native_workflows.rs` para "consertar" a listagem — a próxima compilação sobrescreve.
