# 🐛 BugHunter — espaço no campo de mensagem não movia o cursor

## 💡 What

`draw_composer` quebrava o rascunho com `wrap_text`, a mesma função usada para
renderizar o transcript. `wrap_text` faz `text.split_whitespace()`, então o
espaço digitado no fim do texto sumia — e como o cursor é posicionado a partir da
largura da última linha quebrada, ele ficava parado até a letra seguinte chegar.
Espaços repetidos no meio (`"a  b"`) também eram colapsados para um só.

Correção: `wrap_input` (`crates/tupi-tui/src/run.rs:2146`), que percorre
caractere a caractere, conta cada espaço como coluna e só escolhe **onde**
quebrar (preferindo o último espaço da linha para não cortar palavra ao meio).
`draw_composer` (`crates/tupi-tui/src/run.rs:936`) passou a chamá-la.

## 🎯 Why

O composer é um editor de texto, não markdown: colapsar espaço é correto no
transcript e errado no campo de digitação. O sintoma relatado ("o espaço só
aparece quando digito a próxima letra") era exatamente o cursor parado.

Antes do fix, os dois testes novos reprovavam:

```
run::tests::trailing_space_moves_the_composer_cursor
  left: Position { x: 5, y: 1 }  ·  right: Position { x: 6, y: 1 }
run::tests::repeated_spaces_are_rendered_in_the_composer
  "│› a b                 │"
```

## ✅ Verification

```bash
cargo test -p tupi-tui --lib run::tests::trailing_space_moves_the_composer_cursor
cargo test -p tupi-tui --lib run::tests::repeated_spaces_are_rendered_in_the_composer
cargo test -p tupi-tui --lib      # 90 passed
cargo clippy -p tupi-tui          # 0 erros; 2 avisos pré-existentes
```

## Observações fora do escopo (suíte do workspace)

Falhas alheias a esta mudança, reproduzidas sem tocar nos arquivos:

- `cargo test -p tupi --bin tupi` — `prompt_config::tests::the_checked_in_file_is_valid`:
  `configs/prompts.ron` tem a regra 7 (`/sdd <slug>`) e `tupi_app::prompt::TUPI_SYSTEM`
  não. Fixture versionada desatualizada, mesmo padrão do journal de 2026-11-16.
- `cargo test -p tupi-tools --lib` — `tools::websearch::tests::temp_live_network_probe`
  (sonda de rede, instável por natureza).
- `cargo test -p tupi-kernel --lib` isolado não compila (`#[tokio::test]` sem a
  dev-dependency `tokio` com `macros`); via workspace as features são unificadas e
  compila.

Nada disso foi alterado nesta sessão.
