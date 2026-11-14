# Journal — The Spark 💡

Só ideias radicais e pivots. Nada de log de rotina.

## Sem data registrada (sessão 1) - Tupi Attest: o recibo do código escrito por IA

> Nenhuma tool desta sessão expõe o relógio; a data fica em aberto em vez de
> inventada.

**The "Crazy" Idea:** O Tupi não vende o chat: vende o **recibo**. Para cada
afirmação de um PR gerado por agente, um arquivo de KBs mostra o que foi
provado com hash/tool e o que é **palpite**. `DeliveryLedger`
(`crates/tupi-tools/src/support/ledger.rs:104-267`) passa de otimizador de
tokens a notário: mesmo dado, outro mercado.

**The Hidden Potential:** Todo o supply chain de software já tem attestation de
*como foi construído* (SLSA, in-toto, sigstore) e **nenhuma** de *como foi
escrito*. O comprador (segurança, plataforma, auditoria) já tem orçamento e já
exige selo. E o Tupi é provider-agnóstico por arquitetura — quem escreve não
pode ser o próprio notário. Inverte o modelo: de assinatura de assistente para
infraestrutura de confiança, com todo CLI de IA como parceiro, não concorrente.

**First Step:** `tupi attest --range origin/main..HEAD` rodando em 3 PRs reais
deste repo, em modo replay do ledger (hash registrado vs. hash atual). Sinal
único que importa: alguém pedir o recibo no template de PR sem eu pedir.
