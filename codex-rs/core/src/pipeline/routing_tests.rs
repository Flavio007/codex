use super::*;
use pretty_assertions::assert_eq;

#[test]
fn parses_direct_route() {
    let response = "ROUTE: DIRECT\n\nVou implementar a alteração diretamente no arquivo mod.rs...";
    let decision = parse_routing_decision(response);
    assert_eq!(decision.is_direct(), true);
    assert_eq!(decision.is_delegate(), false);
}

#[test]
fn parses_direct_route_default_when_unformatted() {
    let response = "Certamente! Aqui está a solução para o problema solicitado:";
    let decision = parse_routing_decision(response);
    assert_eq!(decision.is_direct(), true);
}

#[test]
fn parses_delegate_route_with_tasks() {
    let response = r#"
ROUTE: DELEGATE
PLAN_SUMMARY: Dividir a implementação em módulo de tipos e módulo de parser.
TASKS:
---
TASK_NAME: create_types
COMPLEXITY: trivial
RELEVANT_CONTEXT: Estruturas sob pipeline/
TASK: Criar os structs básicos de configuração e roteamento.
CONSTRAINTS: Usar serde e clippy rules.
ACCEPTANCE_TESTS: cargo test -p codex-core
---
TASK_NAME: implement_parser
COMPLEXITY: normal
RELEVANT_CONTEXT: prompts e respostas do Astra
TASK: Implementar o parser de ROUTE: DIRECT e ROUTE: DELEGATE.
CONSTRAINTS: Suportar entradas multilinha.
ACCEPTANCE_TESTS: Testes unitários do parser passando.
---
"#;

    let decision = parse_routing_decision(response);
    let expected = RoutingDecision::Delegate {
        plan_summary: "Dividir a implementação em módulo de tipos e módulo de parser.".to_string(),
        tasks: vec![
            Subtask {
                name: "create_types".to_string(),
                complexity: WorkerComplexity::Trivial,
                relevant_context: "Estruturas sob pipeline/".to_string(),
                task: "Criar os structs básicos de configuração e roteamento.".to_string(),
                constraints: "Usar serde e clippy rules.".to_string(),
                acceptance_tests: "cargo test -p codex-core".to_string(),
            },
            Subtask {
                name: "implement_parser".to_string(),
                complexity: WorkerComplexity::Normal,
                relevant_context: "prompts e respostas do Astra".to_string(),
                task: "Implementar o parser de ROUTE: DIRECT e ROUTE: DELEGATE.".to_string(),
                constraints: "Suportar entradas multilinha.".to_string(),
                acceptance_tests: "Testes unitários do parser passando.".to_string(),
            },
        ],
    };

    assert_eq!(decision, expected);
}

#[test]
fn worker_complexity_model_mapping() {
    assert_eq!(WorkerComplexity::Trivial.default_model(), "gpt-5.6-luna");
    assert_eq!(WorkerComplexity::Normal.default_model(), "gpt-5.6-terra");
    assert_eq!(WorkerComplexity::Difficult.default_model(), "gpt-5.6-sol");

    assert_eq!(
        WorkerComplexity::Normal.resolve_model("custom-terra"),
        "custom-terra"
    );
}
