use anyhow::{Context as _, Result, anyhow};
use credentials_provider::CredentialsProvider;
use editor::Editor;
use futures::{FutureExt, StreamExt, future::BoxFuture};
use gpui::{
    AnyView, App, AsyncApp, ClickEvent, Context, Entity, Subscription, Task, Window,
};
use http_client::HttpClient;
use language_model::{
    AuthenticateError, LanguageModel, LanguageModelCompletionError, LanguageModelCompletionEvent,
    LanguageModelId, LanguageModelName, LanguageModelProvider, LanguageModelProviderId,
    LanguageModelProviderName, LanguageModelProviderState, LanguageModelRequest,
    LanguageModelToolChoice, RateLimiter,
};
use open_ai::ResponseStreamEvent;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsStore};
use std::sync::Arc;
use ui::{IconName, prelude::*};
use util::ResultExt;

use crate::AllLanguageModelSettings;
use crate::provider::open_ai::{OpenAiEventMapper, count_open_ai_tokens, into_open_ai};

const PROVIDER_ID: &str = "azure_openai";
const PROVIDER_NAME: &str = "Azure OpenAI";

// Azure OpenAI response structures that handle null content with tool calls
#[derive(Serialize, Deserialize, Debug)]
pub struct AzureOpenAiResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<AzureOpenAiChoice>,
    pub usage: open_ai::Usage,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AzureOpenAiChoice {
    pub index: u32,
    pub message: AzureOpenAiRequestMessage,
    pub finish_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum AzureOpenAiRequestMessage {
    Assistant {
        content: Option<String>, // Azure OpenAI can return null content with tool calls
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<open_ai::ToolCall>,
    },
    User {
        content: String,
    },
    System {
        content: String,
    },
    Tool {
        content: String,
        tool_call_id: String,
    },
}

// Azure OpenAI request structure that handles both max_tokens and max_completion_tokens
#[derive(Serialize)]
struct AzureOpenAiRequest {
    model: String,
    messages: Vec<open_ai::RequestMessage>,
    #[serde(default)]
    stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stop: Vec<String>,
    temperature: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_choice: Option<open_ai::ToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tools: Vec<open_ai::ToolDefinition>,
}

fn convert_to_azure_request(request: open_ai::Request, model: &open_ai::Model) -> AzureOpenAiRequest {
    // For o1 and o3 models, use max_completion_tokens instead of max_tokens
    let is_reasoning_model = request.model.starts_with("o1") || request.model.starts_with("o3");
    
    // Only set parallel_tool_calls if the model supports it and tools are present
    let parallel_tool_calls = if model.supports_parallel_tool_calls() && !request.tools.is_empty() {
        request.parallel_tool_calls
    } else {
        None
    };
    
    AzureOpenAiRequest {
        model: request.model,
        messages: request.messages,
        stream: request.stream,
        max_tokens: if is_reasoning_model { None } else { request.max_completion_tokens.map(|t| t as u32) },
        max_completion_tokens: if is_reasoning_model { request.max_completion_tokens.map(|t| t as u32) } else { None },
        stop: request.stop,
        temperature: request.temperature,
        tool_choice: request.tool_choice,
        parallel_tool_calls,
        tools: request.tools,
    }
}



#[derive(Default, Clone, Debug, PartialEq)]
pub struct AzureOpenAiSettings {
    pub resource_name: String,
    pub api_version: String,
    pub available_models: Vec<AvailableModel>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AvailableModel {
    pub name: String,
    pub deployment_name: String,
    pub display_name: Option<String>,
    pub max_tokens: u64,
    pub max_output_tokens: Option<u64>,
    pub max_completion_tokens: Option<u64>,
}

pub struct AzureOpenAiLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: gpui::Entity<State>,
}

pub struct State {
    api_key: Option<String>,
    api_key_from_env: bool,
    _subscription: Subscription,
}

const AZURE_OPENAI_API_KEY_VAR: &str = "AZURE_OPENAI_API_KEY";

impl State {
    fn is_authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    fn reset_api_key(&self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        let settings = &AllLanguageModelSettings::get_global(cx).azure_openai;
        let api_url = format!("https://{}.openai.azure.com", settings.resource_name);
        
        cx.spawn(async move |this, cx| {
            credentials_provider
                .delete_credentials(&api_url, &cx)
                .await
                .log_err();
            this.update(cx, |this, cx| {
                this.api_key = None;
                this.api_key_from_env = false;
                cx.notify();
            })
        })
    }

    fn set_api_key(&mut self, api_key: String, cx: &mut Context<Self>) -> Task<Result<()>> {
        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        let settings = &AllLanguageModelSettings::get_global(cx).azure_openai;
        let api_url = format!("https://{}.openai.azure.com", settings.resource_name);
        
        cx.spawn(async move |this, cx| {
            credentials_provider
                .write_credentials(&api_url, "api-key", api_key.as_bytes(), &cx)
                .await
                .log_err();
            this.update(cx, |this, cx| {
                this.api_key = Some(api_key);
                cx.notify();
            })
        })
    }

    fn authenticate(&self, cx: &mut Context<Self>) -> Task<Result<(), AuthenticateError>> {
        if self.is_authenticated() {
            return Task::ready(Ok(()));
        }

        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        let settings = &AllLanguageModelSettings::get_global(cx).azure_openai;
        let api_url = format!("https://{}.openai.azure.com", settings.resource_name);

        cx.spawn(async move |this, cx| {
            let (api_key, from_env) = if let Ok(api_key) = std::env::var(AZURE_OPENAI_API_KEY_VAR) {
                (api_key, true)
            } else {
                let (_, api_key) = credentials_provider
                    .read_credentials(&api_url, &cx)
                    .await?
                    .ok_or(AuthenticateError::CredentialsNotFound)?;
                (
                    String::from_utf8(api_key).context("invalid Azure OpenAI API key")?,
                    false,
                )
            };
            this.update(cx, |this, cx| {
                this.api_key = Some(api_key);
                this.api_key_from_env = from_env;
                cx.notify();
            })?;

            Ok(())
        })
    }
}

impl AzureOpenAiLanguageModelProvider {
    pub fn new(http_client: Arc<dyn HttpClient>, cx: &mut App) -> Self {
        let state = cx.new(|cx| State {
            api_key: None,
            api_key_from_env: false,
            _subscription: cx.observe_global::<SettingsStore>(|_this: &mut State, cx| {
                cx.notify();
            }),
        });

        Self { http_client, state }
    }

    fn create_language_model(&self, model: open_ai::Model, deployment_name: String) -> Arc<dyn LanguageModel> {
        Arc::new(AzureOpenAiLanguageModel {
            id: LanguageModelId::from(model.id().to_string()),
            model,
            deployment_name,
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }
}

impl LanguageModelProviderState for AzureOpenAiLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<gpui::Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for AzureOpenAiLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        LanguageModelProviderId(PROVIDER_ID.into())
    }

    fn name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName(PROVIDER_NAME.into())
    }

    fn icon(&self) -> IconName {
        IconName::AiOpenAi
    }

    fn default_model(&self, cx: &App) -> Option<Arc<dyn LanguageModel>> {
        let settings = &AllLanguageModelSettings::get_global(cx).azure_openai;
        
        settings.available_models.first().map(|available_model| {
            self.create_language_model(
                open_ai::Model::Custom {
                    name: available_model.name.clone(),
                    display_name: available_model.display_name.clone(),
                    max_tokens: available_model.max_tokens,
                    max_output_tokens: available_model.max_output_tokens,
                    max_completion_tokens: available_model.max_completion_tokens,
                },
                available_model.deployment_name.clone(),
            )
        })
    }

    fn default_fast_model(&self, cx: &App) -> Option<Arc<dyn LanguageModel>> {
        self.default_model(cx)
    }

    fn provided_models(&self, cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        let settings = &AllLanguageModelSettings::get_global(cx).azure_openai;
        
        settings
            .available_models
            .iter()
            .map(|available_model| {
                self.create_language_model(
                    open_ai::Model::Custom {
                        name: available_model.name.clone(),
                        display_name: available_model.display_name.clone(),
                        max_tokens: available_model.max_tokens,
                        max_output_tokens: available_model.max_output_tokens,
                        max_completion_tokens: available_model.max_completion_tokens,
                    },
                    available_model.deployment_name.clone(),
                )
            })
            .collect()
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        self.state.update(cx, |state, cx| state.authenticate(cx))
    }

    fn configuration_view(&self, window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| ConfigurationView::new(self.state.clone(), window, cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
        self.state.update(cx, |state, cx| state.reset_api_key(cx))
    }
}

pub struct AzureOpenAiLanguageModel {
    id: LanguageModelId,
    model: open_ai::Model,
    deployment_name: String,
    state: gpui::Entity<State>,
    http_client: Arc<dyn HttpClient>,
    #[allow(dead_code)]
    request_limiter: RateLimiter,
}

impl AzureOpenAiLanguageModel {
    fn stream_completion(
        &self,
        request: open_ai::Request,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<futures::stream::BoxStream<'static, Result<ResponseStreamEvent>>>> {
        let http_client = self.http_client.clone();
        let deployment_name = self.deployment_name.clone();
        let model = self.model.clone();
        let state = self.state.clone();

        // Extract values synchronously to avoid capturing AsyncApp references
        let result = state.read_with(cx, |state, cx| {
            let settings = AllLanguageModelSettings::get_global(cx).azure_openai.clone();
            (state.api_key.clone(), settings)
        });

        async move {
            let (api_key, settings) = result.map_err(|err| anyhow::anyhow!("Failed to read state: {}", err))?;
            
            let api_key = api_key.ok_or_else(|| anyhow::anyhow!("Azure OpenAI API key is not set"))?;
            
            let api_url = format!(
                "https://{}.openai.azure.com/openai/deployments/{}/chat/completions?api-version={}",
                settings.resource_name, deployment_name, settings.api_version
            );

            azure_stream_completion(
                http_client.as_ref(),
                &api_url,
                &api_key,
                request,
                &model,
            ).await
        }
        .boxed()
    }
}

impl LanguageModel for AzureOpenAiLanguageModel {
    fn id(&self) -> LanguageModelId {
        self.id.clone()
    }

    fn name(&self) -> LanguageModelName {
        LanguageModelName::from(self.model.display_name().to_string())
    }

    fn provider_id(&self) -> LanguageModelProviderId {
        LanguageModelProviderId(PROVIDER_ID.into())
    }

    fn provider_name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName(PROVIDER_NAME.into())
    }

    fn supports_tools(&self) -> bool {
        // Check model-specific tool support - o1 models don't support tools
        if self.model.id().starts_with("o1") {
            return false;
        }
        
        // Custom models need explicit tool support configuration
        match &self.model {
            open_ai::Model::Custom { .. } => {
                // For custom models, we could check configuration, but for now assume support
                true
            }
            _ => true,
        }
    }

    fn supports_images(&self) -> bool {
        true
    }

    fn supports_tool_choice(&self, choice: LanguageModelToolChoice) -> bool {
        // Only support tool choice if the model supports tools
        if !self.supports_tools() {
            return false;
        }
        
        match choice {
            LanguageModelToolChoice::Auto
            | LanguageModelToolChoice::Any
            | LanguageModelToolChoice::None => true,
        }
    }

    fn telemetry_id(&self) -> String {
        format!("azure_openai/{}", self.model.id())
    }

    fn max_token_count(&self) -> u64 {
        self.model.max_token_count()
    }

    fn max_output_tokens(&self) -> Option<u64> {
        self.model.max_output_tokens()
    }

    fn count_tokens(
        &self,
        request: LanguageModelRequest,
        cx: &App,
    ) -> BoxFuture<'static, Result<u64>> {
        count_open_ai_tokens(request, self.model.clone(), cx)
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<
            futures::stream::BoxStream<
                'static,
                Result<LanguageModelCompletionEvent, LanguageModelCompletionError>,
            >,
            LanguageModelCompletionError,
        >,
    > {
        let request = into_open_ai(
            request, 
            &self.model.id(), 
            self.model.supports_parallel_tool_calls(), 
            self.model.max_output_tokens()
        );
        let completions = self.stream_completion(request, cx);
        async move {
            let mapper = OpenAiEventMapper::new();
            Ok(mapper.map_stream(completions.await.map_err(|err| LanguageModelCompletionError::Other(err))?).boxed())
        }
        .boxed()
    }
}

// Azure-specific stream completion function
async fn azure_stream_completion(
    client: &dyn HttpClient,
    api_url: &str,
    api_key: &str,
    request: open_ai::Request,
    model: &open_ai::Model,
) -> Result<futures::stream::BoxStream<'static, Result<ResponseStreamEvent>>> {
    use futures::{AsyncBufReadExt, AsyncReadExt, io::BufReader, stream, future};
    use http_client::{AsyncBody, Method, Request as HttpRequest};

    // For o1 models, use non-streaming completion
    if request.model.starts_with("o1") {
        log::debug!("Using non-streaming completion for o1 model: {}", request.model);
        let response = azure_complete(client, api_url, api_key, request, model).await?;
        return Ok(stream::once(future::ready(Ok(response))).boxed());
    }

    // Convert OpenAI request to Azure-compatible request
    let azure_request = convert_to_azure_request(request, model);

    let request_body = serde_json::to_string(&azure_request)
        .context("Failed to serialize Azure OpenAI request")?;

    log::debug!("Azure OpenAI request URL: {}", api_url);
    log::debug!("Azure OpenAI request body: {}", request_body);

    let request_builder = HttpRequest::builder()
        .method(Method::POST)
        .uri(api_url)
        .header("Content-Type", "application/json")
        .header("api-key", api_key);  // Azure uses "api-key" header instead of "Authorization"

    let request = request_builder.body(AsyncBody::from(request_body))?;
    let mut response = client.send(request).await
        .context("Failed to send request to Azure OpenAI API")?;
    
    log::debug!("Azure OpenAI response status: {}", response.status());
    
    if response.status().is_success() {
        let reader = BufReader::new(response.into_body());
        Ok(reader
            .lines()
            .filter_map(|line| async move {
                match line {
                    Ok(line) => {
                        let line = line.trim();
                        
                        // Skip empty lines
                        if line.is_empty() {
                            return None;
                        }
                        
                        let line = line.strip_prefix("data: ")?;
                        if line == "[DONE]" {
                            log::debug!("Azure OpenAI stream completed with [DONE]");
                            None
                        } else {
                            log::debug!("Azure OpenAI stream chunk: {}", line);
                            match serde_json::from_str(line) {
                                Ok(open_ai::ResponseStreamResult::Ok(response)) => {
                                    Some(Ok(response))
                                }
                                Ok(open_ai::ResponseStreamResult::Err { error }) => {
                                    log::error!("Azure OpenAI stream API error: {}", error);
                                    Some(Err(anyhow!("Azure OpenAI API error: {}", error)))
                                }
                                Err(error) => {
                                    log::warn!("Failed to parse Azure OpenAI stream chunk: {} - Error: {}", line, error);
                                    // Don't terminate the stream on parse errors, just log and continue
                                    None
                                }
                            }
                        }
                    }
                    Err(error) => {
                        log::error!("Error reading line from Azure OpenAI stream: {}", error);
                        Some(Err(anyhow!("Stream read error: {}", error)))
                    }
                }
            })
            .boxed())
    } else {
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        log::error!("Azure OpenAI API error - Status: {}, Body: {}", response.status(), body);
        
        // Try to parse Azure-specific error format
        #[derive(Deserialize)]
        struct AzureOpenAiErrorResponse {
            error: AzureOpenAiError,
        }

        #[derive(Deserialize)]
        struct AzureOpenAiError {
            message: String,
            #[serde(rename = "type")]
            error_type: Option<String>,
            code: Option<String>,
        }

        match serde_json::from_str::<AzureOpenAiErrorResponse>(&body) {
            Ok(error_response) => {
                let error_msg = format!(
                    "Azure OpenAI API error: {} (type: {}, code: {})",
                    error_response.error.message,
                    error_response.error.error_type.unwrap_or_default(),
                    error_response.error.code.unwrap_or_default()
                );
                anyhow::bail!(error_msg);
            }
            Err(_) => {
                anyhow::bail!("Azure OpenAI API error: {} {}", response.status(), body);
            }
        }
    }
}

// Azure-specific non-streaming completion function for o1 models
async fn azure_complete(
    client: &dyn HttpClient,
    api_url: &str,
    api_key: &str,
    request: open_ai::Request,
    model: &open_ai::Model,
) -> Result<ResponseStreamEvent> {
    use futures::AsyncReadExt;
    use http_client::{AsyncBody, Method, Request as HttpRequest};

    // Convert OpenAI request to Azure-compatible request (without streaming)
    let mut azure_request = convert_to_azure_request(request, model);
    azure_request.stream = false;

    let request_body = serde_json::to_string(&azure_request)
        .context("Failed to serialize Azure OpenAI request")?;

    log::debug!("Azure OpenAI non-streaming request URL: {}", api_url);
    log::debug!("Azure OpenAI non-streaming request body: {}", request_body);

    let request_builder = HttpRequest::builder()
        .method(Method::POST)
        .uri(api_url)
        .header("Content-Type", "application/json")
        .header("api-key", api_key);

    let request = request_builder.body(AsyncBody::from(request_body))?;
    let mut response = client.send(request).await
        .context("Failed to send request to Azure OpenAI API")?;

    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;

    log::debug!("Azure OpenAI non-streaming response status: {}", response.status());
    log::debug!("Azure OpenAI non-streaming response body: {}", body);

    if response.status().is_success() {
        let azure_response: AzureOpenAiResponse = serde_json::from_str(&body)
            .context("Failed to parse Azure OpenAI response")?;
        
        if azure_response.choices.is_empty() {
            anyhow::bail!("Azure OpenAI response contained no choices. Response body: {}", body);
        }
        
        Ok(adapt_azure_response_to_stream(azure_response))
    } else {
        // Try to parse Azure-specific error format
        #[derive(Deserialize)]
        struct AzureOpenAiErrorResponse {
            error: AzureOpenAiError,
        }

        #[derive(Deserialize)]
        struct AzureOpenAiError {
            message: String,
            #[serde(rename = "type")]
            error_type: Option<String>,
            code: Option<String>,
        }

        match serde_json::from_str::<AzureOpenAiErrorResponse>(&body) {
            Ok(error_response) => {
                let error_msg = format!(
                    "Azure OpenAI API error: {} (type: {}, code: {})",
                    error_response.error.message,
                    error_response.error.error_type.unwrap_or_default(),
                    error_response.error.code.unwrap_or_default()
                );
                anyhow::bail!(error_msg);
            }
            Err(_) => {
                anyhow::bail!("Azure OpenAI API error: {} {}", response.status(), body);
            }
        }
    }
}

fn adapt_azure_response_to_stream(azure_response: AzureOpenAiResponse) -> ResponseStreamEvent {
    ResponseStreamEvent {
        model: azure_response.model,
        choices: azure_response
            .choices
            .into_iter()
            .map(|choice| {
                let mut text_content = String::new();
                let tool_calls = match &choice.message {
                    AzureOpenAiRequestMessage::Assistant { content, tool_calls } => {
                        if let Some(content) = content {
                            text_content.push_str(content);
                        }
                        Some(tool_calls)
                    }
                    AzureOpenAiRequestMessage::User { content } => {
                        text_content.push_str(content);
                        None
                    }
                    AzureOpenAiRequestMessage::System { content } => {
                        text_content.push_str(content);
                        None
                    }
                    AzureOpenAiRequestMessage::Tool { content, .. } => {
                        text_content.push_str(content);
                        None
                    }
                };

                // Convert tool calls to the streaming format
                let stream_tool_calls = if let Some(tool_calls) = tool_calls {
                    if tool_calls.is_empty() {
                        None
                    } else {
                        Some(
                            tool_calls
                                .iter()
                                .enumerate()
                                .map(|(index, tool_call)| open_ai::ToolCallChunk {
                                    index,
                                    id: Some(tool_call.id.clone()),
                                    function: match &tool_call.content {
                                        open_ai::ToolCallContent::Function { function } => {
                                            Some(open_ai::FunctionChunk {
                                                name: Some(function.name.clone()),
                                                arguments: Some(function.arguments.clone()),
                                            })
                                        }
                                    },
                                })
                                .collect(),
                        )
                    }
                } else {
                    None
                };

                open_ai::ChoiceDelta {
                    index: choice.index,
                    delta: open_ai::ResponseMessageDelta {
                        role: Some(match choice.message {
                            AzureOpenAiRequestMessage::Assistant { .. } => open_ai::Role::Assistant,
                            AzureOpenAiRequestMessage::User { .. } => open_ai::Role::User,
                            AzureOpenAiRequestMessage::System { .. } => open_ai::Role::System,
                            AzureOpenAiRequestMessage::Tool { .. } => open_ai::Role::Tool,
                        }),
                        content: if text_content.is_empty() {
                            None
                        } else {
                            Some(text_content)
                        },
                        tool_calls: stream_tool_calls,
                    },
                    finish_reason: choice.finish_reason,
                }
            })
            .collect(),
        usage: Some(azure_response.usage),
    }
}

struct ConfigurationView {
    api_key_editor: Entity<Editor>,
    state: gpui::Entity<State>,
    load_credentials_task: Option<Task<()>>,
}

impl ConfigurationView {
    fn new(state: gpui::Entity<State>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let api_key_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Enter your Azure OpenAI API key", cx);
            editor
        });

        let mut this = Self {
            api_key_editor,
            state: state.clone(),
            load_credentials_task: None,
        };

        this.load_credentials(window, cx);
        this
    }

    fn load_credentials(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let _api_key_editor = self.api_key_editor.clone();
        
        self.load_credentials_task = Some(cx.spawn(async move |_, cx| {
            if let Some(_api_key) = state.read_with(cx, |state, _| state.api_key.clone()).ok().flatten() {
                // We can't easily set the text in the async context without window access
                // This would need to be handled differently in a real implementation
            }
        }));
    }

    fn save_api_key(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let api_key = self.api_key_editor.read(cx).text(cx);
        if !api_key.trim().is_empty() {
            self.state.update(cx, |state, cx| {
                state.set_api_key(api_key.trim().to_string(), cx).detach();
            });
        }
    }

    fn reset_api_key(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.api_key_editor.update(cx, |editor, cx| {
            editor.set_text("", window, cx);
        });
        self.state.update(cx, |state, cx| {
            state.reset_api_key(cx).detach();
        });
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const INSTRUCTIONS: [&str; 4] = [
            "To use Azure OpenAI models, you need to configure:",
            "1. Your Azure OpenAI resource name in settings",
            "2. API version (e.g., '2023-03-15-preview')",
            "3. Your API key below",
        ];

        v_flex()
            .size_full()
            .gap_2()
            .child(
                v_flex()
                    .gap_1()
                    .children(INSTRUCTIONS.iter().map(|instruction| {
                        Label::new(*instruction)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                    })),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("API Key").size(LabelSize::Small))
                    .child(self.api_key_editor.clone())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("save_api_key", "Save API Key")
                                    .style(ButtonStyle::Filled)
                                    .on_click(cx.listener(Self::save_api_key)),
                            )
                            .child(
                                Button::new("reset_api_key", "Reset")
                                    .style(ButtonStyle::Subtle)
                                    .on_click(cx.listener(Self::reset_api_key)),
                            ),
                    ),
            )
    }
} 