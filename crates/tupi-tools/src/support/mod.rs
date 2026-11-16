pub mod ast;
pub(crate) mod backtrack;
pub(crate) mod budget;
// O contrato denso nasce aqui e é consumido pela migração das tools de lista;
// o `allow` sai junto com o último chamador.
#[allow(dead_code)]
pub(crate) mod dense;
pub mod graph;
pub(crate) mod ledger;
pub(crate) mod paths;
pub(crate) mod proc;
pub(crate) mod ranking;
pub mod scanner;
pub(crate) mod span;
pub(crate) mod store;
pub(crate) mod terms;
pub(crate) mod web;
