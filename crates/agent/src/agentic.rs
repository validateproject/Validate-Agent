use std::env;

use anyhow::{anyhow, bail, Context, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionNamedToolChoice, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        ChatCompletionRequestUserMessageContent, ChatCompletionToolArgs,
        ChatCompletionToolChoiceOption, ChatCompletionToolType, CreateChatCompletionRequestArgs,
        FunctionName, FunctionObjectArgs,
    },
    Client,
};
use common::{
    Action, AgenticConfig, IssueKind, OpenAiAgentConfig, Playbook, ValidatorConfig, ValidatorId,
    ValidatorMetrics,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::debug;

const DEFAULT_SYSTEM_PROMPT: &str = r#"System: You are Validator Copilot, an SRE operator for Solana validators.

Begin with a concise checklist (3-7 bullets) of what you will do; keep items conceptual, not implementation-level.

Your task: Analyze the provided metrics and the issue. Respond with STRICT JSON matching the schema below. Do not wrap your response in markdown fences. Select at least one appropriate action and keep your plans minimal and safe. If user impact is medium or higher, always include a send_alert step in actions.

## Output Format
Return a strictly valid JSON object with the following keys, in this order:
- "playbook_id": string (required)
- "rationale": short sentence as a string (required)
- "actions": array of objects (at least one; required). Each action object includes:
    - "kind": string; must be one of "disable_rpc", "enable_rpc", "restart_validator", "throttle_rpc_client", "run_maintenance_script", or "send_alert" (required)
    - "message": string; required only for kind "send_alert" (omit otherwise)
    - "script_name": string; required only for kind "run_maintenance_script" (omit otherwise)

Validation: After constructing your response, validate that all required fields are present, in the proper order, and correctly formatted. If any required fields are missing, out of order, malformed, or if kind is unrecognized, or if a kind-specific required key (such as message for send_alert or script_name for run_maintenance_script) is absent, flag the response as invalid and do not proceed."#;

const TOOL_NAME: &str = "propose_remediation_plan";

const DEFAULT_OBJECTIVES: &[&str] = &[
    "Protect validator health and uptime.",
    "Prefer reversible or low-risk actions before disruptive ones.",
    "Communicate impact to operators when taking disruptive steps.",
];

const DEFAULT_ACTION_LIBRARY: &[PromptAction] = &[
    PromptAction {
        name: "disable_rpc",
        description: "Temporarily disable the public RPC endpoint while remediation is running.",
        required_fields: &[],
    },
    PromptAction {
        name: "enable_rpc",
        description: "Re-enable the public RPC endpoint once the validator is stable.",
        required_fields: &[],
    },
    PromptAction {
        name: "restart_validator",
        description: "Restart the validator process to clear unhealthy state.",
        required_fields: &[],
    },
    PromptAction {
        name: "throttle_rpc_client",
        description: "Throttle incoming RPC traffic to reduce load or protect the cluster.",
        required_fields: &[],
    },
    PromptAction {
        name: "run_maintenance_script",
        description: "Execute a maintenance script (e.g., cleanup-logs.sh). Provide script_name.",
        required_fields: &["script_name"],
    },
    PromptAction {
        name: "send_alert",
        description: "Notify operators about the issue and remediation steps. Provide message.",
        required_fields: &["message"],
    },
];

const DEFAULT_TEMPERATURE: f32 = 0.2;
const DEFAULT_API_BASE: &str = "https://api.openai.com/v1";
const DEFAULT_API_KEY_ENV: &str = "OPENAI_API_KEY";

#[derive(Clone, Debug)]
pub struct AgenticBrain {
    planner: Planner,
}

#[derive(Clone, Debug)]
enum Planner {
    Disabled,
    OpenAi(OpenAiPlanner),
}

#[derive(Clone, Debug)]
struct OpenAiPlanner {
    client: Client<OpenAIConfig>,
    model: String,
    system_prompt: String,
    temperature: f32,
}

#[derive(Clone, Debug)]
pub struct AgenticDecision {
    pub playbook: Playbook,
    pub rationale: Option<String>,
}

#[derive(Serialize)]
struct PromptPayload<'a> {
    issue: IssueKind,
    metrics: &'a ValidatorMetrics,
    validator: PromptValidator<'a>,
    objectives: &'static [&'static str],
    actions: &'static [PromptAction],
}

#[derive(Serialize)]
struct PromptValidator<'a> {
    id: &'a str,
    host: &'a str,
    prometheus_url: &'a str,
}

#[derive(Serialize)]
struct PromptAction {
    name: &'static str,
    description: &'static str,
    required_fields: &'static [&'static str],
}

#[derive(Debug, Deserialize)]
struct LlmPlan {
    #[serde(default)]
    playbook_id: String,
    #[serde(default)]
    rationale: Option<String>,
    actions: Vec<LlmActionSpec>,
}

#[derive(Debug, Deserialize)]
struct LlmActionSpec {
    kind: LlmActionKind,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    script_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LlmActionKind {
    DisableRpc,
    EnableRpc,
    RestartValidator,
    ThrottleRpcClient,
    RunMaintenanceScript,
    SendAlert,
}

impl AgenticBrain {
    pub fn new(cfg: Option<AgenticConfig>) -> Result<Self> {
        let planner = match cfg {
            Some(agentic_cfg) => Planner::try_from(agentic_cfg)?,
            None => Planner::Disabled,
        };
        Ok(Self { planner })
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self.planner, Planner::OpenAi(_))
    }

    pub async fn plan(
        &self,
        validator: &ValidatorConfig,
        metrics: &ValidatorMetrics,
        issue: IssueKind,
    ) -> Result<Option<AgenticDecision>> {
        match &self.planner {
            Planner::Disabled => Ok(None),
            Planner::OpenAi(planner) => planner.plan(validator, metrics, issue).await,
        }
    }
}

impl Planner {
    fn try_from(cfg: AgenticConfig) -> Result<Self> {
        match cfg {
            AgenticConfig::OpenAi(inner) => Ok(Self::OpenAi(OpenAiPlanner::try_new(inner)?)),
        }
    }
}

impl OpenAiPlanner {
    fn try_new(cfg: OpenAiAgentConfig) -> Result<Self> {
        let env_key = cfg
            .api_key_env
            .clone()
            .unwrap_or_else(|| DEFAULT_API_KEY_ENV.to_string());
        let api_key = env::var(&env_key).with_context(|| {
            format!("environment variable {env_key} is required to use the OpenAI agentic provider")
        })?;

        let openai_cfg = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(cfg.api_base.as_deref().unwrap_or(DEFAULT_API_BASE));

        let client = Client::with_config(openai_cfg);
        let system_prompt = cfg
            .system_prompt
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

        Ok(Self {
            client,
            model: cfg.model,
            system_prompt,
            temperature: cfg.temperature.unwrap_or(DEFAULT_TEMPERATURE),
        })
    }

    async fn plan(
        &self,
        validator: &ValidatorConfig,
        metrics: &ValidatorMetrics,
        issue: IssueKind,
    ) -> Result<Option<AgenticDecision>> {
        let payload = PromptPayload {
            issue,
            metrics,
            validator: PromptValidator {
                id: &validator.id.0,
                host: &validator.host,
                prometheus_url: &validator.prometheus_url,
            },
            objectives: DEFAULT_OBJECTIVES,
            actions: DEFAULT_ACTION_LIBRARY,
        };
        let user_payload =
            serde_json::to_string(&payload).context("failed to serialize prompt payload")?;

        let system_msg = ChatCompletionRequestSystemMessageArgs::default()
            .content(self.system_prompt.clone())
            .build()
            .context("failed to build system prompt message")?;
        let user_msg = ChatCompletionRequestUserMessageArgs::default()
            .content(ChatCompletionRequestUserMessageContent::Text(user_payload))
            .build()
            .context("failed to build user prompt message")?;

        let tool = ChatCompletionToolArgs::default()
            .function(
                FunctionObjectArgs::default()
                    .name(TOOL_NAME)
                    .description("Produce a validator remediation plan that matches the strict JSON schema.")
                    .parameters(json!({
                        "type": "object",
                        "properties": {
                            "playbook_id": { "type": "string", "minLength": 1 },
                            "rationale": { "type": "string", "minLength": 1 },
                            "actions": {
                                "type": "array",
                                "minItems": 1,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "kind": {
                                            "type": "string",
                                            "enum": [
                                                "disable_rpc",
                                                "enable_rpc",
                                                "restart_validator",
                                                "throttle_rpc_client",
                                                "run_maintenance_script",
                                                "send_alert"
                                            ]
                                        },
                                        "message": { "type": "string" },
                                        "script_name": { "type": "string" }
                                    },
                                    "required": ["kind"],
                                    "additionalProperties": false,
                                    "allOf": [
                                        {
                                            "if": { "properties": { "kind": { "const": "send_alert" } } },
                                            "then": { "required": ["message"] }
                                        },
                                        {
                                            "if": { "properties": { "kind": { "const": "run_maintenance_script" } } },
                                            "then": { "required": ["script_name"] }
                                        }
                                    ]
                                }
                            }
                        },
                        "required": ["playbook_id", "rationale", "actions"],
                        "additionalProperties": false
                    }))
                    .build()
                    .context("failed to build function definition")?,
            )
            .build()
            .context("failed to build tool definition")?;

        let tool_choice = ChatCompletionToolChoiceOption::Named(ChatCompletionNamedToolChoice {
            r#type: ChatCompletionToolType::Function,
            function: FunctionName {
                name: TOOL_NAME.to_string(),
            },
        });

        let request = CreateChatCompletionRequestArgs::default()
            .model(self.model.clone())
            .temperature(self.temperature)
            .messages(vec![
                ChatCompletionRequestMessage::System(system_msg),
                ChatCompletionRequestMessage::User(user_msg),
            ])
            .tools(vec![tool])
            .tool_choice(tool_choice)
            .build()
            .context("failed to build OpenAI chat completion request")?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .context("OpenAI chat completion failed")?;
        let Some(choice) = response.choices.first() else {
            return Ok(None);
        };

        if let Some(tool_calls) = &choice.message.tool_calls {
            for call in tool_calls {
                if call.r#type == ChatCompletionToolType::Function
                    && call.function.name == TOOL_NAME
                {
                    let args = call.function.arguments.clone();
                    debug!(
                        validator = validator.id.0,
                        tool = TOOL_NAME,
                        arguments = args.as_str(),
                        "agentic provider tool response"
                    );
                    let plan =
                        parse_plan_payload(&args).context("failed to parse tool call payload")?;
                    if plan.actions.is_empty() {
                        return Ok(None);
                    }
                    let decision = plan.into_decision(issue, &validator.id)?;
                    return Ok(Some(decision));
                }
            }
        }

        let raw = choice.message.content.clone().unwrap_or_default();
        debug!(
            validator = validator.id.0,
            raw_response = raw.as_str(),
            "agentic provider response"
        );

        let plan = parse_plan_payload(&raw).context("failed to parse OpenAI response payload")?;
        if plan.actions.is_empty() {
            return Ok(None);
        }
        let decision = plan.into_decision(issue, &validator.id)?;
        Ok(Some(decision))
    }
}

fn parse_plan_payload(raw: &str) -> Result<LlmPlan> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("empty agentic response");
    }
    match serde_json::from_str::<LlmPlan>(trimmed) {
        Ok(plan) => Ok(plan),
        Err(primary) => {
            // Try to locate the first JSON object in case the model wrapped it in prose.
            if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
                if start < end {
                    let slice = &trimmed[start..=end];
                    return serde_json::from_str::<LlmPlan>(slice)
                        .map_err(|_| anyhow!(primary))
                        .context("unable to parse JSON plan from agentic response");
                }
            }
            Err(anyhow!(primary)).context("agentic response was not valid JSON")
        }
    }
}

impl LlmPlan {
    fn into_decision(self, issue: IssueKind, validator: &ValidatorId) -> Result<AgenticDecision> {
        let id = if self.playbook_id.trim().is_empty() {
            format!("agentic-{issue:?}")
                .replace(' ', "-")
                .to_lowercase()
        } else {
            self.playbook_id
        };
        let steps = self
            .actions
            .into_iter()
            .map(|action| action.into_action(validator))
            .collect::<Result<Vec<_>>>()?;
        if steps.is_empty() {
            bail!("agentic plan did not include any actions");
        }
        Ok(AgenticDecision {
            playbook: Playbook {
                id,
                trigger: issue,
                steps,
            },
            rationale: self.rationale.filter(|r| !r.trim().is_empty()),
        })
    }
}

impl LlmActionSpec {
    fn into_action(self, validator: &ValidatorId) -> Result<Action> {
        let v = validator.clone();
        let action = match self.kind {
            LlmActionKind::DisableRpc => Action::DisableRpc { validator: v },
            LlmActionKind::EnableRpc => Action::EnableRpc { validator: v },
            LlmActionKind::RestartValidator => Action::RestartValidator { validator: v },
            LlmActionKind::ThrottleRpcClient => Action::ThrottleRpcClient { validator: v },
            LlmActionKind::RunMaintenanceScript => Action::RunMaintenanceScript {
                validator: v,
                script_name: self
                    .script_name
                    .filter(|s| !s.trim().is_empty())
                    .context("run_maintenance_script requires script_name")?,
            },
            LlmActionKind::SendAlert => Action::SendAlert {
                validator: v,
                message: self
                    .message
                    .filter(|s| !s.trim().is_empty())
                    .context("send_alert requires message")?,
            },
        };
        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{IssueKind, ValidatorId};

    fn validator_id() -> ValidatorId {
        ValidatorId("validator-test".into())
    }

    #[test]
    fn parses_clean_json_payload() {
        let raw = r#"{
            "playbook_id": "plan-123",
            "rationale": "Restart to clear slot lag.",
            "actions": [
                {"kind": "disable_rpc"},
                {"kind": "restart_validator"},
                {"kind": "send_alert", "message": "Restarting validator to clear slot lag"}
            ]
        }"#;
        let plan = parse_plan_payload(raw).expect("plan parsed");
        let decision = plan
            .into_decision(IssueKind::SlotLagHigh, &validator_id())
            .expect("decision");
        assert_eq!(decision.playbook.steps.len(), 3);
    }

≥⁄⁄    #[test]
    fn extracts_json_from_code_fence() {
        let raw = "Here you go:\n```json\n{\"playbook_id\":\"abc\",\"actions\":[{\"kind\":\"disable_rpc\"},{\"kind\":\"send_alert\",\"message\":\"done\"}]}\n```";
        let plan = parse_plan_payload(raw).expect("parse from fence");
        assert_eq!(plan.actions.len(), 2);
    }

    #[test]
    fn rejects_missing_required_fields() {
        let raw = r#"{"actions":[{"kind":"run_maintenance_script"}]}"#;
        let plan = parse_plan_payload(raw).expect("parsed");
        assert!(plan
            .into_decision(IssueKind::HardwareOverload, &validator_id())
            .is_err());
    }
}
