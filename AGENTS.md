# AGENTS.md

## Economia de tokens é requisito, não otimização

Toda mudança que adiciona tokens a um request precisa dizer quais tokens
ela evita. O histórico é dívida: cada linha anexada é paga de novo em
todo passo seguinte. Prefira índice no prefixo cacheado a round trip que
cresce o histórico. Nenhuma afirmação entra no contexto sem um
falsificador: se nenhuma tool pode dizer "está errado", aquilo é palpite.

## Fontes de verdade

- Tool ganha de memória do modelo e ganha do índice.
- O índice do repositório é um mapa do início da sessão, não o conteúdo dos arquivos.
- Corpo de arquivo só entra no contexto por `read`, e não se repete o que a tool já disse que está inalterado.
