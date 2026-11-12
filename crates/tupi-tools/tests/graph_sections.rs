use tupi_tools::graph::{
    GraphBuilder, ProjectCodeGraph, RelationEdge, RelationKind, SectionId, SectionKind, SymbolNode,
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

#[test]
fn builder_extracts_container_with_fields_and_belongs_to() {
    let dir = tempfile::tempdir().unwrap();
    let auth_rs = r#"
pub struct TokenService {
    secret: String,
}

pub struct AuthService {
    token_service: TokenService,
}

impl AuthService {
    pub fn login(&self) -> bool {
        true
    }

    pub fn logout(&self) {}
}
"#;
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/auth.rs"), auth_rs).unwrap();

    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();

    let auth_idx = graph
        .graph
        .node_indices()
        .find(|&idx| {
            graph.graph[idx].name == "AuthService"
                && graph.graph[idx].kind == SectionKind::Container
        })
        .expect("AuthService container node should exist");

    let members = graph.members_of(auth_idx);
    assert_eq!(
        members.len(),
        2,
        "AuthService must have login and logout as members"
    );

    let member_names: Vec<&str> = members
        .iter()
        .map(|&idx| graph.graph[idx].name.as_str())
        .collect();
    assert!(member_names.contains(&"login"));
    assert!(member_names.contains(&"logout"));

    let login_idx = members
        .into_iter()
        .find(|&idx| graph.graph[idx].name == "login")
        .unwrap();
    assert_eq!(graph.container_of(login_idx), Some(auth_idx));
}

#[test]
fn resolver_resolves_self_method_and_field_method() {
    let dir = tempfile::tempdir().unwrap();
    let code = r#"
pub struct TokenService {
    secret: String,
}

impl TokenService {
    pub fn decode(&self) -> bool {
        true
    }
}

pub struct AuthService {
    token: TokenService,
}

impl AuthService {
    pub fn internal_check(&self) -> bool {
        true
    }

    pub fn login(&self) -> bool {
        let ok = self.internal_check();
        let valid = self.token.decode();
        ok && valid
    }
}
"#;
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/auth.rs"), code).unwrap();

    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();

    let login_callees = graph.callees("login");
    let callee_names: Vec<&str> = login_callees.iter().map(|d| d.name.as_str()).collect();

    assert!(
        callee_names.contains(&"internal_check"),
        "self.internal_check() must resolve to sibling method: {callee_names:?}"
    );
    assert!(
        callee_names.contains(&"decode"),
        "self.token.decode() must resolve via field type TokenService: {callee_names:?}"
    );
}
