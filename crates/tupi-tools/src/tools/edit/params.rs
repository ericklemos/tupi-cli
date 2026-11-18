use serde::Deserialize;
use tupi_kernel::ToolError;

/// Rotas do `edit`, nomeadas na ordem em que a recusa as reporta. São exclusivas:
/// nenhuma tem precedência sobre outra, e a mistura de duas é recusa — não
/// escolha implícita.
const ROUTES: &str = "`symbol`+`new_body` (símbolo de AST, só Rust), `start`+`end`+`expect`+`new` (faixa de linhas com o texto atual esperado) ou `old`+`new` (âncora literal única, qualquer arquivo de texto)";

#[derive(Debug, Deserialize)]
pub struct EditParams {
    pub path: String,
    /// Rota AST (Rust): nome simples (`verify`) ou qualificado (`Auth::verify`).
    pub symbol: Option<String>,
    /// Rota literal: trecho exato que sai; precisa casar uma única vez no arquivo.
    pub old: Option<String>,
    /// Texto novo: na rota literal substitui `old`, na faixa de linhas substitui
    /// `start..=end` (vazio apaga o trecho).
    pub new: Option<String>,
    /// Rota faixa de linhas: primeira linha, contada a partir de 1, inclusive.
    pub start: Option<usize>,
    /// Rota faixa de linhas: última linha, contada a partir de 1, inclusive.
    pub end: Option<usize>,
    /// Rota faixa de linhas: o texto atual de `start..=end`, copiado do `read`
    /// sem a numeração e sem a quebra de linha final. Vazio só quando a faixa
    /// são linhas em branco; ausente é recusa, porque faixa sem expectativa é
    /// escrita às cegas.
    pub expect: Option<String>,
    pub expected_hash: String,
    /// Rota AST: novo bloco `{ … }` do símbolo.
    pub new_body: Option<String>,
    pub signature: Option<String>,
    pub budget_tokens: Option<usize>,
}

/// Endereço da edição, decidido antes de tocar no disco: faixa de linhas com o
/// texto atual esperado (texto puro, para linhas repetidas e arquivos grandes),
/// âncora literal única (caminho padrão, qualquer texto) ou símbolo de AST
/// (otimização opcional, só Rust).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditAddress {
    Symbol {
        name: String,
        new_body: String,
        signature: Option<String>,
    },
    Lines {
        start: usize,
        end: usize,
        expect: String,
        new: String,
    },
    Literal {
        old: String,
        new: String,
    },
}

impl EditParams {
    /// Decide a rota, sem escolha implícita. Há três, exclusivas: `symbol`+`new_body`,
    /// `start`+`end`+`expect`+`new` e `old`+`new`. Zero endereço é recusa com a
    /// lista, dois endereços é recusa que nomeia a regra — nenhuma rota tem
    /// precedência sobre outra.
    pub fn address(&self) -> Result<EditAddress, ToolError> {
        let symbol = self
            .symbol
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let has_old = self.old.is_some();
        let old = self.old.as_deref().filter(|anchor| !anchor.is_empty());

        if self.symbol.is_some() && symbol.is_none() {
            return Err(invalid(
                "`symbol` veio vazio: informe o nome do símbolo ou troque de rota com `old`+`new`",
            ));
        }
        if has_old && old.is_none() {
            return Err(invalid(
                "`old` veio vazio: a âncora é o trecho literal exato que precisa casar uma única vez; use `symbol` se quer endereçar por nome",
            ));
        }

        let range = self.lines()?;

        match (symbol, old, range) {
            (None, None, None) => Err(invalid(&format!("falta o endereço: use {ROUTES}"))),
            (Some(name), None, None) => {
                if self.old.is_some() || self.new.is_some() {
                    return Err(invalid(
                        "`old`/`new` só valem juntos: com `symbol`, o novo conteúdo vai em `new_body`",
                    ));
                }
                let new_body = self
                    .new_body
                    .as_deref()
                    .filter(|body| !body.trim().is_empty())
                    .ok_or_else(|| {
                        invalid("`symbol` exige `new_body`: o bloco `{ … }` completo que substitui o corpo")
                    })?;
                Ok(EditAddress::Symbol {
                    name: name.to_string(),
                    new_body: new_body.to_string(),
                    signature: self.signature.clone(),
                })
            }
            (None, None, Some((start, end, expect))) => {
                if self.new_body.is_some() {
                    return Err(invalid(
                        "`new_body` pertence à rota `symbol`: com faixa de linhas o texto novo vai em `new`",
                    ));
                }
                if self.signature.is_some() {
                    return Err(invalid(
                        "`signature` substitui a cabeça de um símbolo e só existe na rota `symbol`; com faixa de linhas escreva a cabeça dentro de `new`",
                    ));
                }
                let new = self.new.clone().ok_or_else(|| {
                    invalid("faixa de linhas exige `new`: o texto que substitui `start..=end` (vazio apaga o trecho)")
                })?;
                Ok(EditAddress::Lines {
                    start,
                    end,
                    expect,
                    new,
                })
            }
            (None, Some(old), None) => {
                if self.new_body.is_some() {
                    return Err(invalid(
                        "`new_body` pertence à rota `symbol`: com `old` o texto novo vai em `new`",
                    ));
                }
                if self.signature.is_some() {
                    return Err(invalid(
                        "`signature` substitui a cabeça de um símbolo e só existe na rota `symbol`; com `old`+`new` escreva a cabeça dentro de `old`/`new`",
                    ));
                }
                let new = self.new.clone().ok_or_else(|| {
                    invalid("`old` exige `new`: o texto que entra no lugar da âncora")
                })?;
                Ok(EditAddress::Literal {
                    old: old.to_string(),
                    new,
                })
            }
            _ => {
                let mut routes = Vec::new();
                if symbol.is_some() {
                    routes.push("`symbol`+`new_body`");
                }
                if self.start.is_some() || self.end.is_some() || self.expect.is_some() {
                    routes.push("`start`+`end`+`expect`+`new`");
                }
                if old.is_some() {
                    routes.push("`old`+`new`");
                }
                Err(invalid(&format!(
                    "{} são rotas exclusivas, sem precedência entre si: envie só uma. Use {ROUTES}",
                    routes.join(" e ")
                )))
            }
        }
    }

    /// Normaliza a faixa de linhas: 1-based, inclusiva, sempre com a expectativa
    /// do texto atual. Ponta solta, ordem invertida, linha 0 e faixa sem `expect`
    /// são recusa — faixa sem expectativa é escrita às cegas.
    fn lines(&self) -> Result<Option<(usize, usize, String)>, ToolError> {
        match (self.start, self.end) {
            (None, None) => {
                if self.expect.is_some() {
                    return Err(invalid(
                        "`expect` só vale com `start`+`end`: ele fixa o texto atual da faixa que sai",
                    ));
                }
                Ok(None)
            }
            (Some(start), Some(end)) => {
                if start == 0 || end == 0 {
                    return Err(invalid(
                        "linha é contada a partir de 1, como `caminho:linha` do `read`: `start`/`end` zero não existe",
                    ));
                }
                if start > end {
                    return Err(invalid(&format!(
                        "`start` ({start}) vem depois de `end` ({end}): a faixa é inclusiva, então `start` ≤ `end`"
                    )));
                }
                let expect = self.expect.clone().ok_or_else(|| {
                    invalid("faixa de linhas exige `expect`: o texto atual de `start..=end`, copiado do `read` sem a numeração. Sem expectativa é escrita às cegas")
                })?;
                Ok(Some((start, end, expect)))
            }
            (Some(_), None) => Err(invalid(
                "`start` exige `end`: a faixa é inclusiva e precisa das duas pontas",
            )),
            (None, Some(_)) => Err(invalid(
                "`end` exige `start`: a faixa é inclusiva e precisa das duas pontas",
            )),
        }
    }
}

fn invalid(message: &str) -> ToolError {
    ToolError::InvalidArguments(format!("{message}. Nada foi escrito."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> EditParams {
        EditParams {
            path: "guia.md".into(),
            symbol: None,
            old: None,
            new: None,
            start: None,
            end: None,
            expect: None,
            expected_hash: "0123456789ab".into(),
            new_body: None,
            signature: None,
            budget_tokens: None,
        }
    }

    fn lines(start: usize, end: usize, expect: &str) -> EditParams {
        EditParams {
            start: Some(start),
            end: Some(end),
            expect: Some(expect.into()),
            new: Some("novo".into()),
            ..params()
        }
    }

    fn refusal(params: &EditParams) -> String {
        match params.address() {
            Ok(address) => panic!("esperava recusa, veio {address:?}"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn a_line_range_is_addressed_with_its_expectation() {
        assert_eq!(
            lines(3, 4, "a: 1\nb: 2").address().unwrap(),
            EditAddress::Lines {
                start: 3,
                end: 4,
                expect: "a: 1\nb: 2".into(),
                new: "novo".into(),
            }
        );
        // `new` vazio apaga a faixa; `expect` vazio vale só para linhas em branco.
        let blank = EditParams {
            new: Some(String::new()),
            expect: Some(String::new()),
            ..lines(2, 2, "")
        };
        assert!(matches!(
            blank.address().unwrap(),
            EditAddress::Lines { new, .. } if new.is_empty()
        ));
    }

    #[test]
    fn a_line_range_without_expectation_is_refused_as_blind() {
        let blind = EditParams {
            expect: None,
            ..lines(1, 2, "")
        };
        let message = refusal(&blind);
        assert!(message.contains("`expect`"), "{message}");
        assert!(message.contains("cegas"), "{message}");
        assert!(message.contains("Nada foi escrito"), "{message}");
    }

    #[test]
    fn half_a_range_is_refused() {
        let only_start = EditParams {
            end: None,
            ..lines(1, 2, "x")
        };
        assert!(refusal(&only_start).contains("`start` exige `end`"));
        let only_end = EditParams {
            start: None,
            ..lines(1, 2, "x")
        };
        assert!(refusal(&only_end).contains("`end` exige `start`"));
        let stray_expect = EditParams {
            expect: Some("x".into()),
            ..params()
        };
        assert!(refusal(&stray_expect).contains("só vale com `start`+`end`"));
    }

    #[test]
    fn an_inverted_or_zero_based_range_is_refused() {
        assert!(refusal(&lines(5, 2, "x")).contains("`start` (5) vem depois de `end` (2)"));
        assert!(refusal(&lines(0, 2, "x")).contains("a partir de 1"));
    }

    #[test]
    fn a_range_never_borrows_the_symbol_fields() {
        let with_body = EditParams {
            new_body: Some("{}".into()),
            ..lines(1, 2, "x")
        };
        assert!(refusal(&with_body).contains("`new_body` pertence à rota `symbol`"));
        let with_signature = EditParams {
            signature: Some("fn f()".into()),
            ..lines(1, 2, "x")
        };
        assert!(refusal(&with_signature).contains("`signature`"));
        let without_new = EditParams {
            new: None,
            ..lines(1, 2, "x")
        };
        assert!(refusal(&without_new).contains("exige `new`"));
    }

    #[test]
    fn two_routes_are_refused_with_the_rule_and_both_names() {
        let range_and_literal = EditParams {
            old: Some("a: 1".into()),
            new: Some("a: 2".into()),
            ..lines(1, 2, "a: 1")
        };
        let message = refusal(&range_and_literal);
        assert!(
            message.contains("rotas exclusivas, sem precedência"),
            "{message}"
        );
        assert!(message.contains("`start`+`end`+`expect`+`new`"), "{message}");
        assert!(message.contains("`old`+`new`"), "{message}");
        assert!(message.contains("envie só uma"), "{message}");

        let range_and_symbol = EditParams {
            symbol: Some("verify".into()),
            new_body: Some("{\n}".into()),
            ..lines(1, 2, "a: 1")
        };
        let message = refusal(&range_and_symbol);
        assert!(message.contains("`symbol`+`new_body`"), "{message}");
        assert!(message.contains("`start`+`end`+`expect`+`new`"), "{message}");
    }

    #[test]
    fn the_literal_and_symbol_routes_keep_their_own_rules() {
        let literal = EditParams {
            old: Some("a: 1".into()),
            new: Some("a: 2".into()),
            ..params()
        };
        assert_eq!(
            literal.address().unwrap(),
            EditAddress::Literal {
                old: "a: 1".into(),
                new: "a: 2".into(),
            }
        );

        let symbol = EditParams {
            symbol: Some("verify".into()),
            new_body: Some("{\n}".into()),
            signature: Some("pub fn verify(x: u8) -> bool".into()),
            ..params()
        };
        assert!(matches!(
            symbol.address().unwrap(),
            EditAddress::Symbol { name, .. } if name == "verify"
        ));

        let both = EditParams {
            symbol: Some("verify".into()),
            old: Some("a: 1".into()),
            new: Some("a: 2".into()),
            new_body: Some("{\n}".into()),
            ..params()
        };
        assert!(refusal(&both).contains("`symbol`+`new_body` e `old`+`new`"));

        assert!(refusal(&params()).contains("falta o endereço"));
    }
}
