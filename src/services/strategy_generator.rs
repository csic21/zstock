use std::collections::BTreeSet;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::market::Market;
use crate::domain::strategy::{
    CompiledStrategy, StrategySpec, ValidationError, local_templates, strategy_id,
};

use super::llm::{LlmClient, LlmRequest, LlmResponse};

pub const STRATEGY_GENERATOR_PROMPT_VERSION: &str = "strategy-generator-v1";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_STRATEGIES: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyGenerationInput {
    pub research_goal: String,
    pub market: Market,
    pub timeframe: String,
    pub universe_snapshot_id: String,
    pub universe_description: String,
    pub interval_description: String,
    pub risk_limits: String,
    pub cost_assumptions: String,
    pub requested_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftSource {
    AiInitial,
    AiRepair,
    LocalFallback,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedStrategyDraft {
    pub strategy_id: String,
    pub spec: StrategySpec,
    pub source: DraftSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyGenerationError {
    pub stage: String,
    pub candidate_index: Option<usize>,
    pub message: String,
    pub validation_errors: Vec<ValidationError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyBatchDraft {
    pub strategies: Vec<GeneratedStrategyDraft>,
    pub requested_count: usize,
    pub raw_candidate_count: usize,
    pub validation_failure_count: usize,
    pub duplicate_count: usize,
    pub repair_attempted: bool,
    pub model: String,
    pub transport: String,
    pub prompt_version: String,
    pub raw_response_sha256: Option<String>,
    pub raw_response_summary: Option<String>,
    pub errors: Vec<StrategyGenerationError>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyEnvelope {
    strategies: Vec<StrategySpec>,
}

pub struct StrategyGenerator<'a> {
    client: &'a dyn LlmClient,
}

impl<'a> StrategyGenerator<'a> {
    pub const fn new(client: &'a dyn LlmClient) -> Self {
        Self { client }
    }

    pub fn generate(&self, input: &StrategyGenerationInput) -> StrategyBatchDraft {
        let requested_count = normalized_count(input.requested_count);
        let mut draft = empty_draft(requested_count);
        let request = LlmRequest {
            system: system_prompt().into(),
            user: generation_prompt(input, requested_count),
            max_output_tokens: 8_000,
        };
        let initial = match self.client.complete(&request) {
            Ok(response) => response,
            Err(error) => {
                draft.errors.push(StrategyGenerationError {
                    stage: "transport".into(),
                    candidate_index: None,
                    message: error.to_string(),
                    validation_errors: vec![],
                });
                fill_with_local_templates(&mut draft, &input.universe_snapshot_id);
                return draft;
            }
        };
        apply_response_metadata(&mut draft, &initial);
        let first = parse_candidates(&initial.text);
        let (initial_valid, initial_errors, raw_count) = match first {
            Ok(specs) => validate_candidates(specs, DraftSource::AiInitial),
            Err(error) => (
                vec![],
                vec![StrategyGenerationError {
                    stage: "parse".into(),
                    candidate_index: None,
                    message: error.to_string(),
                    validation_errors: vec![],
                }],
                0,
            ),
        };
        draft.raw_candidate_count = raw_count;
        draft.validation_failure_count = initial_errors.len();
        draft.errors.extend(initial_errors.clone());
        insert_unique(&mut draft, initial_valid);

        if !initial_errors.is_empty() || draft.strategies.len() < requested_count {
            draft.repair_attempted = true;
            let repair = LlmRequest {
                system: repair_system_prompt().into(),
                user: repair_prompt(input, requested_count, &initial.text, &initial_errors),
                max_output_tokens: 8_000,
            };
            match self.client.complete(&repair) {
                Ok(response) => {
                    draft.model = response.model.clone();
                    draft.transport = response.transport.clone();
                    match parse_candidates(&response.text) {
                        Ok(specs) => {
                            let (valid, errors, raw_count) =
                                validate_candidates(specs, DraftSource::AiRepair);
                            draft.raw_candidate_count += raw_count;
                            draft.validation_failure_count += errors.len();
                            draft.errors.extend(errors);
                            insert_unique(&mut draft, valid);
                        }
                        Err(error) => draft.errors.push(StrategyGenerationError {
                            stage: "repair_parse".into(),
                            candidate_index: None,
                            message: error.to_string(),
                            validation_errors: vec![],
                        }),
                    }
                }
                Err(error) => draft.errors.push(StrategyGenerationError {
                    stage: "repair_transport".into(),
                    candidate_index: None,
                    message: error.to_string(),
                    validation_errors: vec![],
                }),
            }
        }
        fill_with_local_templates(&mut draft, &input.universe_snapshot_id);
        draft.strategies.truncate(requested_count);
        draft
    }
}

fn empty_draft(requested_count: usize) -> StrategyBatchDraft {
    StrategyBatchDraft {
        strategies: vec![],
        requested_count,
        raw_candidate_count: 0,
        validation_failure_count: 0,
        duplicate_count: 0,
        repair_attempted: false,
        model: "local-template".into(),
        transport: "local".into(),
        prompt_version: STRATEGY_GENERATOR_PROMPT_VERSION.into(),
        raw_response_sha256: None,
        raw_response_summary: None,
        errors: vec![],
    }
}

fn normalized_count(requested: usize) -> usize {
    if requested == 0 {
        5
    } else {
        requested.clamp(3, MAX_STRATEGIES)
    }
}

fn parse_candidates(response: &str) -> Result<Vec<StrategySpec>> {
    if response.len() > MAX_RESPONSE_BYTES {
        bail!("strategy response exceeds {MAX_RESPONSE_BYTES} byte limit");
    }
    let json = extract_json_object(response)?;
    let envelope: StrategyEnvelope = serde_json::from_str(json)?;
    if envelope.strategies.is_empty() {
        bail!("strategy batch is empty");
    }
    if envelope.strategies.len() > MAX_STRATEGIES {
        bail!("strategy batch exceeds maximum of {MAX_STRATEGIES}");
    }
    Ok(envelope.strategies)
}

fn extract_json_object(response: &str) -> Result<&str> {
    let start = response
        .find('{')
        .ok_or_else(|| anyhow!("no JSON object found"))?;
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, character) in response[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + offset + character.len_utf8();
                    return Ok(&response[start..end]);
                }
            }
            _ => {}
        }
    }
    bail!("unterminated JSON object")
}

fn validate_candidates(
    specs: Vec<StrategySpec>,
    source: DraftSource,
) -> (
    Vec<GeneratedStrategyDraft>,
    Vec<StrategyGenerationError>,
    usize,
) {
    let raw_count = specs.len();
    let mut valid = Vec::new();
    let mut errors = Vec::new();
    for (index, spec) in specs.into_iter().enumerate() {
        match CompiledStrategy::compile(spec.clone()) {
            Ok(compiled) => valid.push(GeneratedStrategyDraft {
                strategy_id: compiled.strategy_id().into(),
                spec,
                source,
            }),
            Err(validation_errors) => errors.push(StrategyGenerationError {
                stage: if source == DraftSource::AiRepair {
                    "repair_validation".into()
                } else {
                    "validation".into()
                },
                candidate_index: Some(index),
                message: "strategy failed deterministic local validation".into(),
                validation_errors,
            }),
        }
    }
    (valid, errors, raw_count)
}

fn insert_unique(draft: &mut StrategyBatchDraft, strategies: Vec<GeneratedStrategyDraft>) {
    let mut seen: BTreeSet<_> = draft
        .strategies
        .iter()
        .map(|item| item.strategy_id.clone())
        .collect();
    for strategy in strategies {
        if seen.insert(strategy.strategy_id.clone()) {
            draft.strategies.push(strategy);
        } else {
            draft.duplicate_count += 1;
        }
    }
}

fn fill_with_local_templates(draft: &mut StrategyBatchDraft, universe_id: &str) {
    let local = local_templates(universe_id)
        .into_iter()
        .map(|spec| GeneratedStrategyDraft {
            strategy_id: strategy_id(&spec),
            spec,
            source: DraftSource::LocalFallback,
        })
        .collect();
    insert_unique(draft, local);
    draft.strategies.truncate(draft.requested_count);
}

fn apply_response_metadata(draft: &mut StrategyBatchDraft, response: &LlmResponse) {
    draft.model = response.model.clone();
    draft.transport = response.transport.clone();
    draft.raw_response_sha256 = Some(sha256(response.text.as_bytes()));
    draft.raw_response_summary = Some(response.text.chars().take(500).collect());
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn system_prompt() -> &'static str {
    "You generate safe, diverse daily-bar research strategies as JSON only. Never emit or request executable code. The local validator is authoritative."
}

fn repair_system_prompt() -> &'static str {
    "Repair a rejected strategy JSON batch exactly once. Return JSON only and obey every schema and range limit. Do not explain or emit code."
}

fn generation_prompt(input: &StrategyGenerationInput, count: usize) -> String {
    format!(
        "Generate exactly {count} logically diverse strategies. Do not merely vary one period.\n\
Research goal: {}\nMarket: {:?}\nTimeframe: {}\nUniverse snapshot id: {}\n\
Universe: {}\nInterval: {}\nRisk limits: {}\nCosts: {}\n\n{}",
        input.research_goal,
        input.market,
        input.timeframe,
        input.universe_snapshot_id,
        input.universe_description,
        input.interval_description,
        input.risk_limits,
        input.cost_assumptions,
        schema_contract(&input.universe_snapshot_id)
    )
}

fn repair_prompt(
    input: &StrategyGenerationInput,
    count: usize,
    response: &str,
    errors: &[StrategyGenerationError],
) -> String {
    let response: String = response.chars().take(32_000).collect();
    format!(
        "Return a complete replacement batch of exactly {count} strategies.\n\
Universe id must remain {}.\nValidation errors: {}\nRejected response: {}\n{}",
        input.universe_snapshot_id,
        serde_json::to_string(errors).unwrap_or_default(),
        response,
        schema_contract(&input.universe_snapshot_id)
    )
}

fn schema_contract(universe_id: &str) -> String {
    format!(
        r#"Return one JSON object: {{"strategies":[StrategySpec,...]}}.
StrategySpec required keys: schema_version=1, name, hypothesis, timeframe="1d",
universe={{"kind":"dataset_snapshot","id":"{universe_id}"}}, entry, exit, position, metadata.
entry is recursively exactly one of:
{{"all":[entry,...]}}, {{"any":[entry,...]}}, {{"not":entry}},
{{"compare":{{"left":value,"op":"above|below|at_least|at_most|equal","right":value}}}},
{{"crosses_above":{{"left":value,"right":value}}}}, {{"crosses_below":{{"left":value,"right":value}}}}.
value is {{"constant":number}} or one whitelisted indicator object. Indicators:
open/high/low/close/volume with lag; return/sma/ema/rsi/atr/n_day_high/n_day_low with period and lag;
macd with fast_period, slow_period, signal_period, component=line|signal|histogram, lag;
boll with period, std_dev, band=upper|middle|lower, lag.
exit is recursively {{"all":[exit,...]}} or {{"any":[exit,...]}} or one of
{{"hold_days":integer}}, {{"stop_loss_pct":number}}, {{"take_profit_pct":number}},
{{"condition":entry}}. position has size_pct, max_positions, allow_pyramiding=false.
metadata has generator, prompt_version="strategy-generator-v1", optional model and parent_strategy_id.
Hard limits: depth<=6; entry nodes<=32; exit nodes<=32; periods 2..250; lag 0..250;
stop loss 0.5..30%; take profit 0.5..100%; hold days 1..250;
size_pct in (0,100], max_positions 1..100, size_pct*max_positions<=100.
Unknown keys and indicators are forbidden. Output JSON only, with no markdown fence."#
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::domain::strategy::LocalTemplate;

    struct MockClient {
        responses: Mutex<Vec<Result<LlmResponse>>>,
    }

    impl LlmClient for MockClient {
        fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse> {
            self.responses.lock().unwrap().remove(0)
        }
    }

    fn input() -> StrategyGenerationInput {
        StrategyGenerationInput {
            research_goal: "low drawdown".into(),
            market: Market::AShare,
            timeframe: "1d".into(),
            universe_snapshot_id: "dataset-fixture".into(),
            universe_description: "frozen liquid stock snapshot".into(),
            interval_description: "2020..2025 training description only".into(),
            risk_limits: "max drawdown 15%".into(),
            cost_assumptions: "versioned A-share costs".into(),
            requested_count: 3,
        }
    }

    fn response(text: String) -> Result<LlmResponse> {
        Ok(LlmResponse {
            text,
            model: "mock-model".into(),
            transport: "mock".into(),
        })
    }

    fn envelope(specs: Vec<StrategySpec>) -> String {
        serde_json::json!({"strategies": specs}).to_string()
    }

    #[test]
    fn code_fenced_natural_text_is_parsed_but_never_executed_and_duplicates_are_removed() {
        let spec = LocalTemplate::NDayHighBreakout.build("dataset-fixture");
        let client = MockClient {
            responses: Mutex::new(vec![
                response(format!(
                    "Here is JSON, not code:\n```json\n{}\n```",
                    envelope(vec![spec.clone(), spec])
                )),
                response(envelope(local_templates("dataset-fixture")[..3].to_vec())),
            ]),
        };
        let result = StrategyGenerator::new(&client).generate(&input());
        assert_eq!(result.strategies.len(), 3);
        assert!(result.duplicate_count >= 1);
        assert!(result.repair_attempted);
        assert!(
            result
                .strategies
                .iter()
                .all(|strategy| CompiledStrategy::compile(strategy.spec.clone()).is_ok())
        );
    }

    #[test]
    fn invalid_first_response_is_repaired_once() {
        let client = MockClient {
            responses: Mutex::new(vec![
                response("not json".into()),
                response(envelope(local_templates("dataset-fixture")[..3].to_vec())),
            ]),
        };
        let result = StrategyGenerator::new(&client).generate(&input());
        assert_eq!(result.strategies.len(), 3);
        assert!(result.repair_attempted);
        assert!(result.errors.iter().any(|error| error.stage == "parse"));
        assert!(
            result
                .strategies
                .iter()
                .all(|strategy| strategy.source == DraftSource::AiRepair)
        );
    }

    #[test]
    fn two_invalid_responses_fall_back_to_local_templates() {
        let client = MockClient {
            responses: Mutex::new(vec![response("bad".into()), response("still bad".into())]),
        };
        let result = StrategyGenerator::new(&client).generate(&input());
        assert_eq!(result.strategies.len(), 3);
        assert!(
            result
                .strategies
                .iter()
                .all(|strategy| strategy.source == DraftSource::LocalFallback)
        );
        assert!(result.errors.len() >= 2);
    }

    #[test]
    fn unavailable_llm_still_returns_complete_local_experiment_batch() {
        let client = MockClient {
            responses: Mutex::new(vec![Err(anyhow!("offline"))]),
        };
        let result = StrategyGenerator::new(&client).generate(&input());
        assert_eq!(result.strategies.len(), 3);
        assert!(!result.repair_attempted);
        assert_eq!(result.transport, "local");
    }
}
