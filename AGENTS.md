# AGENTS.md

## Economia de tokens é requisito, não otimização

Toda mudança que adiciona tokens a um request precisa dizer quais tokens
ela evita. O histórico é dívida: cada linha anexada é paga de novo em
todo passo seguinte. O prompt não carrega o repositório: a descoberta é
sob demanda por `map`, e CPU/RAM pagam a reconstrução. Nenhuma afirmação
entra no contexto sem um falsificador: se nenhuma tool pode dizer "está
errado", aquilo é palpite.

## Fontes de verdade

- Tool ganha de memória do modelo.
- O prompt não contém o repositório; `map` é o ponto de partida e o grafo de chamadas vive em RAM.
- Corpo de arquivo só entra no contexto por `read`, e não se repete o que a tool já disse que está inalterado.
