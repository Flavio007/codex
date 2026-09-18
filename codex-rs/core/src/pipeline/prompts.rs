use serde_json::json;

use super::architect_selector::ScoutAssessment;
use super::compaction::ArchitectContext;

/// Record of an architect escalation from a lower tier model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationAttempt {
    pub from_model: String,
    pub reason: String,
    pub findings: Vec<String>,
}

/// JSON schema enforced for Scout structured outputs.
pub fn scout_assessment_json_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "handoff": {
                "type": "string",
                "description": "Comprehensive technical summary of findings in # HANDOFF_SUMMARY format."
            },
            "assessment": {
                "type": "object",
                "properties": {
                    "estimated_files": { "type": "integer", "minimum": 1 },
                    "estimated_subsystems": { "type": "integer", "minimum": 1 },
                    "estimated_tasks": { "type": "integer", "minimum": 1 },
                    "cross_cutting": { "type": "boolean" },
                    "architectural_change": { "type": "boolean" },
                    "concurrency_or_unsafe": { "type": "boolean" },
                    "performance_sensitive": { "type": "boolean" },
                    "public_api_change": { "type": "boolean" },
                    "migration_required": { "type": "boolean" },
                    "ambiguity": {
                        "type": "string",
                        "enum": ["low", "medium", "high"]
                    }
                },
                "required": [
                    "estimated_files",
                    "estimated_subsystems",
                    "estimated_tasks",
                    "cross_cutting",
                    "architectural_change",
                    "concurrency_or_unsafe",
                    "performance_sensitive",
                    "public_api_change",
                    "migration_required",
                    "ambiguity"
                ],
                "additionalProperties": false
            }
        },
        "required": ["handoff", "assessment"],
        "additionalProperties": false
    })
}

/// System instructions for the exploration / context builder phase.
pub const CONTEXT_BUILDER_SYSTEM_PROMPT: &str = r#"Você é o Context Builder em um pipeline colaborativo de múltiplos modelos.
Sua missão é investigar exaustivamente o pedido do usuário e o repositório local.

Instruções fundamentais:
1. Investigue o pedido completamente utilizando ferramentas de busca, inspeção de arquivos e análise de código.
2. Localize com precisão:
   - Arquivos relevantes e seus caminhos absolutos/relativos;
   - Símbolos (funções, structs, traits, métodos, tipos);
   - Arquitetura afetada e fluxo de dados;
   - Dependências diretas e indiretas;
   - Bugs, inconsistências ou problemas identificados;
   - Testes relacionados existentes;
   - Restrições arquiteturais, de estilo e de desempenho.
3. NÃO implemente a solução nem altere arquivos de código nesta etapa.
4. Ao concluir toda a investigação, produza a resposta no formato JSON estruturado contendo:
   - "handoff": O relatório técnico completo no formato # HANDOFF_SUMMARY.
   - "assessment": A avaliação quantitativa de sinais de complexidade (estimated_files, estimated_subsystems, estimated_tasks, cross_cutting, architectural_change, concurrency_or_unsafe, performance_sensitive, public_api_change, migration_required, ambiguity).

Seja minucioso, detalhado e conciso nas referências técnicas."#;

/// System instructions for the Architect phase.
pub const ARCHITECT_SYSTEM_PROMPT: &str = r#"Você é o Architect em um pipeline colaborativo adaptativo de modelos de IA.
Sua responsabilidade é avaliar criticamente a complexidade da demanda e decidir a estratégia ótima de execução.

Se julgar que a tarefa excede sua capacidade analítica (por exemplo, exigindo redesign global, concorrência complexa ou incerteza crítica não resolvida):
Você pode solicitar escalonamento para o próximo nível de modelo respondendo com:
{
  "status": "escalate",
  "reason": "<motivo específico do escalonamento>",
  "findings": ["<fato ou insight 1>", "<fato ou insight 2>"]
}
(Ou em formato texto: STATUS: ESCALATE\nREASON: <motivo>\nFINDINGS:\n- <fato 1>)

Caso decida prosseguir (status = "ready"):
- Se a tarefa for simples, direta ou envolver poucas alterações concentradas:
  Responda com ROUTE: DIRECT (ou JSON com "status": "ready", "route": "direct") e implemente a solução.
- Se a tarefa for complexa ou decomponível:
  Responda com ROUTE: DELEGATE (ou JSON com "status": "ready", "route": "delegate", "plan_summary": "...", "tasks": [...]).

Classificação de Complexidade dos Workers:
- trivial: mudanças simples, refatorações menores, documentação (executado por worker trivial).
- normal: implementação padrão de componentes, lógica de negócio (executado por worker normal).
- difficult: algoritmos intrincados, concorrência, interfaces críticas (executado por worker difficult)."#;

/// Formats the context builder user input with the user request.
pub fn context_builder_user_prompt(user_prompt: &str) -> String {
    format!(
        "Investigue o seguinte pedido do usuário no repositório:\n\n\
         {user_prompt}\n\n\
         Lembre-se: localize todos os arquivos relevantes, símbolos, arquitetura afetada, dependências, \
         bugs, testes e restrições. NÃO implemente ainda. Conclua gerando o JSON com handoff e assessment."
    )
}

/// Formats the input prompt for the Architect with technical context and optional escalation history.
pub fn architect_context_prompt(
    user_prompt: &str,
    context: &ArchitectContext,
    assessment: &ScoutAssessment,
    previous_escalation: Option<&EscalationAttempt>,
) -> String {
    let mut prompt = format!("### Pedido Original do Usuário\n{user_prompt}\n\n");

    match context {
        ArchitectContext::ExplicitHandoff { handoff } => {
            prompt.push_str(&format!(
                "### Resumo de Investigação (HANDOFF_SUMMARY)\n{handoff}\n\n"
            ));
        }
        ArchitectContext::CompactedHistory { handoff_summary, .. } => {
            prompt.push_str(
                "### Contexto de Investigação\n\
                 O histórico completo da investigação do Scout foi compactado nativamente e está \
                 disponível no histórico desta sessão.\n\n",
            );
            if let Some(summary) = handoff_summary {
                prompt.push_str(&format!("### Destaques do Handoff\n{summary}\n\n"));
            }
        }
    }

    prompt.push_str(&format!(
        "### Avaliação Técnica de Complexidade\n{}\n\n",
        assessment.format_summary()
    ));

    if let Some(esc) = previous_escalation {
        prompt.push_str(&format!(
            "### Tentativa Anterior do Architect ({})\n\
             Motivo da Escalação: {}\n\
             Descobertas Prévias:\n",
            esc.from_model, esc.reason
        ));
        for finding in &esc.findings {
            prompt.push_str(&format!("- {finding}\n"));
        }
        prompt.push('\n');
    }

    prompt.push_str(
        "Avalie o pedido e os dados técnicos. Decida entre prosseguir (`ROUTE: DIRECT` ou `ROUTE: DELEGATE`) \
         ou solicitar escalonamento caso o escopo exceda sua capacidade analítica.",
    );

    prompt
}

/// Formats the isolated task prompt sent to a worker with `fork_turns=\"none\"`.
pub fn worker_task_prompt(
    relevant_context: &str,
    task: &str,
    constraints: &str,
    acceptance_tests: &str,
) -> String {
    format!(
        "### Relevant Context\n\
         {relevant_context}\n\n\
         ### Task\n\
         {task}\n\n\
         ### Constraints\n\
         {constraints}\n\n\
         ### Acceptance Tests\n\
         {acceptance_tests}"
    )
}

