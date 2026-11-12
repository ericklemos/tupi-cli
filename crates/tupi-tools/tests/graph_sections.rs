use tupi_tools::graph::{
    ProjectCodeGraph, RelationEdge, RelationKind, SectionId, SectionKind, SymbolNode,
};

#[test]
fn container_and_members_hierarchy_in_stable_graph() {
    let mut graph = ProjectCodeGraph::new();

    let container_node = SymbolNode {
        name: "Foo".to_string(),
        qualified: "Foo".to_string(),
        relative: "src/foo.rs".to_string(),
        start_line: 1,
        end_line: 10,
        signature: "pub struct Foo".to_string(),
        is_public: true,
        is_callable: false,
        is_test: false,
        language: tupi_tools::ast::Language::Rust,
        kind: SectionKind::Container,
        id: SectionId::new("src/foo.rs", "Foo"),
        hash: blake3::hash(b"struct Foo"),
    };
    let container_idx = graph.graph.add_node(container_node);

    let method1_node = SymbolNode {
        name: "bar".to_string(),
        qualified: "Foo::bar".to_string(),
        relative: "src/foo.rs".to_string(),
        start_line: 3,
        end_line: 5,
        signature: "pub fn bar(&self)".to_string(),
        is_public: true,
        is_callable: true,
        is_test: false,
        language: tupi_tools::ast::Language::Rust,
        kind: SectionKind::Member,
        id: SectionId::new("src/foo.rs", "Foo::bar"),
        hash: blake3::hash(b"fn bar"),
    };
    let method1_idx = graph.graph.add_node(method1_node);

    let method2_node = SymbolNode {
        name: "baz".to_string(),
        qualified: "Foo::baz".to_string(),
        relative: "src/foo.rs".to_string(),
        start_line: 6,
        end_line: 9,
        signature: "pub fn baz(&self)".to_string(),
        is_public: true,
        is_callable: true,
        is_test: false,
        language: tupi_tools::ast::Language::Rust,
        kind: SectionKind::Member,
        id: SectionId::new("src/foo.rs", "Foo::baz"),
        hash: blake3::hash(b"fn baz"),
    };
    let method2_idx = graph.graph.add_node(method2_node);

    graph.graph.add_edge(
        method1_idx,
        container_idx,
        RelationEdge {
            line: 3,
            kind: RelationKind::BelongsTo,
        },
    );
    graph.graph.add_edge(
        method2_idx,
        container_idx,
        RelationEdge {
            line: 6,
            kind: RelationKind::BelongsTo,
        },
    );

    assert_eq!(graph.container_of(method1_idx), Some(container_idx));
    assert_eq!(graph.container_of(method2_idx), Some(container_idx));

    let members = graph.members_of(container_idx);
    assert_eq!(members.len(), 2);
    assert!(members.contains(&method1_idx));
    assert!(members.contains(&method2_idx));

    // StableGraph: remover um nó não afeta o índice dos outros
    graph.graph.remove_node(method1_idx);
    assert_eq!(graph.graph.node_weight(method2_idx).unwrap().name, "baz");
    assert_eq!(graph.members_of(container_idx), vec![method2_idx]);
}
