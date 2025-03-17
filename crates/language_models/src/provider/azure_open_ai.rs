use std::sync::Arc;

use anyhow::{anyhow, Context as _, Result};
use credentials_provider::CredentialsProvider;
use editor::{Editor, EditorElement, EditorStyle};
use futures::{future::BoxFuture, FutureExt, Stream, StreamExt, AsyncBufReadExt, AsyncReadExt};
use gpui::{
    AnyView, App, AsyncApp, Context, Entity, FontStyle, Subscription, Task, TextStyle, WhiteSpace,
};
use http_client::HttpClient;
use language_model::{
    AuthenticateError, LanguageModel, LanguageModelCompletionEvent, LanguageModelId,
    LanguageModelName, LanguageModelProvider, LanguageModelProviderId, LanguageModelProviderName,
    LanguageModelProviderState, LanguageModelRequest, Role,
};
use settings::Settings;
use settings::SettingsStore;
use theme::ThemeSettings;
use ui::prelude::*;
use ui::{Icon, IconName};

use crate::provider::open_ai::count_open_ai_tokens;

pub struct AzureOpenAiLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: gpui::Entity<State>,
}

pub struct State {
    api_key: Option<String>,
    api_url: Option<String>,
    deployment_name: Option<String>,
    api_version: Option<String>,
    _subscription: Subscription,
}

impl State {
    fn is_authenticated(&self) -> bool {
        self.api_key.is_some()
            && self.api_url.is_some()
            && self.deployment_name.is_some()
            && self.api_version.is_some()
    }

    fn reset_credentials(&self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        let api_url = self.api_url.clone().unwrap_or_default();
        cx.spawn(|this, mut cx| async move {
            credentials_provider
                .delete_credentials(&api_url, &cx)
                .await
                .ok();
            this.update(&mut cx, |this, cx| {
                this.api_key = None;
                this.api_url = None;
                this.deployment_name = None;
                this.api_version = None;
                cx.notify();
            })
        })
    }

    fn set_credentials(
        &mut self,
        api_key: String,
        api_url: String,
        deployment_name: String,
        api_version: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        cx.spawn(|this, mut cx| async move {
            credentials_provider
                .write_credentials(&api_url, "", api_key.as_bytes(), &cx)
                .await
                .ok();

            this.update(&mut cx, |this, cx| {
                this.api_key = Some(api_key);
                this.api_url = Some(api_url);
                this.deployment_name = Some(deployment_name);
                this.api_version = Some(api_version);
                cx.notify();
            })
        })
    }

    fn authenticate(&self, cx: &mut Context<Self>) -> Task<Result<(), AuthenticateError>> {
        if self.is_authenticated() {
            return Task::ready(Ok(()));
        }

        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        let api_url = self.api_url.clone().unwrap_or_default();

        cx.spawn(|this, mut cx| async move {
            let (api_key, _from_env) = credentials_provider
                .read_credentials(&api_url, &cx)
                .await?
                .ok_or(AuthenticateError::CredentialsNotFound)?;

            let api_key = String::from_utf8(api_key.into()).context("invalid API key")?;

            this.update(&mut cx, |this, cx| {
                this.api_key = Some(api_key);
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
            api_url: None,
            deployment_name: None,
            api_version: None,
            _subscription: cx.observe_global::<SettingsStore>(|_this: &mut State, cx| {
                cx.notify();
            }),
        });

        Self { http_client, state }
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
        LanguageModelProviderId("azure_openai".into())
    }

    fn name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName("Azure OpenAI".into())
    }

    fn icon(&self) -> IconName {
        IconName::AiOpenAi
    }

    fn default_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(Arc::new(AzureOpenAiLanguageModel {
            id: LanguageModelId::from("azure_openai_model".to_string()),
            name: LanguageModelName::from("Azure OpenAI Model".to_string()),
            state: self.state.clone(),
            http_client: self.http_client.clone(),
        }))
    }

    fn provided_models(&self, _cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        // You can extend this to return multiple models if needed
        vec![Arc::new(AzureOpenAiLanguageModel {
            id: LanguageModelId::from("azure_openai_model".to_string()),
            name: LanguageModelName::from("Azure OpenAI Model".to_string()),
            state: self.state.clone(),
            http_client: self.http_client.clone(),
        })]
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        self.state.update(cx, |state, cx| state.authenticate(cx))
    }

    fn configuration_view(&self, window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| AzureConfigurationView::new(self.state.clone(), window, cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
        self.state.update(cx, |state, cx| state.reset_credentials(cx))
    }
}

pub struct AzureOpenAiLanguageModel {
    id: LanguageModelId,
    name: LanguageModelName,
    state: gpui::Entity<State>,
    http_client: Arc<dyn HttpClient>,
}

impl AzureOpenAiLanguageModel {
    fn stream_completion(
        &self,
        request: language_model::LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<futures::stream::BoxStream<'static, Result<LanguageModelCompletionEvent>>>,
    > {
        let http_client = self.http_client.clone();

        // Retrieve credentials from state.
        let Ok(credentials) = cx.read_entity(&self.state, |state, _cx| {
            let (api_key, api_url, deployment_name, api_version) = state.get_credentials();
            (
                api_key.map(|s| s.to_string()),
                api_url.map(|s| s.to_string()),
                deployment_name.map(|s| s.to_string()),
                api_version.map(|s| s.to_string()),
            )
        }) else {
            return futures::future::ready(Err(anyhow!("App state dropped"))).boxed();
        };

        async move {
            let (api_key, api_url, deployment_name, api_version) = credentials;

            let api_key = api_key.ok_or_else(|| anyhow!("Missing Azure OpenAI API Key"))?;
            let api_url = api_url.ok_or_else(|| anyhow!("Missing Azure OpenAI API URL"))?;
            let deployment_name = deployment_name.ok_or_else(|| anyhow!("Missing Deployment Name"))?;
            let api_version = api_version.ok_or_else(|| anyhow!("Missing API Version"))?;

            // Convert the request and build the JSON payload.
            let open_ai_request = into_azure_open_ai_request(request);
            let request_payload = build_request_payload(&open_ai_request, &deployment_name)?;

            // Determine if non-streaming mode should be used.
            let is_non_streaming = deployment_name == "o1" || deployment_name == "o1-preview" || deployment_name == "o1-mini";

            if is_non_streaming {
                // For non-streaming models.
                let response = complete_azure_open_ai(
                    http_client.as_ref(),
                    &api_url,
                    &api_key,
                    &deployment_name,
                    &api_version,
                    request_payload,
                ).await?;

                let event = adapt_response_to_stream(response);
                let stream = futures::stream::once(futures::future::ready(Ok(event)));
                let text_stream = extract_text_from_azure_events(stream)
                    .map(|result| result.map(LanguageModelCompletionEvent::Text));
                Ok(text_stream.boxed())
            } else {
                // For streaming models.
                let completions = stream_azure_open_ai_completion(
                    http_client.as_ref(),
                    &api_url,
                    &api_key,
                    &deployment_name,
                    &api_version,
                    request_payload,
                ).await?;
                Ok(extract_text_from_azure_events(completions)
                    .map(|result| result.map(LanguageModelCompletionEvent::Text))
                    .boxed())
            }
        }
        .boxed()
    }
}


fn build_request_payload(
    request: &open_ai::Request,
    deployment_name: &str,
) -> Result<serde_json::Value> {
    // Convert the request into a JSON value.
    let mut payload = serde_json::to_value(request)?;
    // If deployment is "o3-mini", add reasoning_effort.
    if deployment_name == "o3-mini" {
        if let serde_json::Value::Object(ref mut map) = payload {
            map.insert(
                "reasoning_effort".to_string(),
                serde_json::Value::String("high".to_string()),
            );
        }
    }
    Ok(payload)
}


async fn complete_azure_open_ai(
    client: &dyn HttpClient,
    api_url: &str,
    api_key: &str,
    deployment_name: &str,
    api_version: &str,
    request_payload: serde_json::Value,
) -> Result<open_ai::Response> {
    let uri = format!(
        "{}/openai/deployments/{}/chat/completions?api-version={}",
        api_url.trim_end_matches('/'),
        deployment_name,
        api_version
    );
    let request_builder = http_client::Request::builder()
        .method(http_client::Method::POST)
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("api-key", api_key);
    let request_body = serde_json::to_string(&request_payload)?;
    let request = request_builder.body(http_client::AsyncBody::from(request_body))?;
    let mut response = client.send(request).await?;
    if response.status().is_success() {
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        let response: open_ai::Response = serde_json::from_str(&body)?;
        Ok(response)
    } else {
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        Err(anyhow!(
            "Failed to connect to Azure OpenAI API: {} {}",
            response.status(),
            body
        ))
    }
}

/// Adapts a non-streaming response into a ResponseStreamEvent.
pub fn adapt_response_to_stream(response: open_ai::Response) -> open_ai::ResponseStreamEvent {
    open_ai::ResponseStreamEvent {
        created: response.created as u32,
        model: response.model,
        choices: response.choices.into_iter().map(|choice| {
            open_ai::ChoiceDelta {
                index: choice.index,
                delta: open_ai::ResponseMessageDelta {
                    role: Some(match choice.message {
                        open_ai::RequestMessage::Assistant { .. } => open_ai::Role::Assistant,
                        open_ai::RequestMessage::User { .. } => open_ai::Role::User,
                        open_ai::RequestMessage::System { .. } => open_ai::Role::System,
                        open_ai::RequestMessage::Tool { .. } => open_ai::Role::Tool,
                    }),
                    content: match choice.message {
                        open_ai::RequestMessage::Assistant { content, .. } => content,
                        open_ai::RequestMessage::User { content } => Some(content),
                        open_ai::RequestMessage::System { content } => Some(content),
                        open_ai::RequestMessage::Tool { content, .. } => Some(content),
                    },
                    tool_calls: None,
                },
                finish_reason: choice.finish_reason,
            }
        }).collect(),
        usage: Some(response.usage),
    }
}

impl LanguageModel for AzureOpenAiLanguageModel {
    fn id(&self) -> LanguageModelId {
        self.id.clone()
    }

    fn name(&self) -> LanguageModelName {
        self.name.clone()
    }

    fn provider_id(&self) -> LanguageModelProviderId {
        LanguageModelProviderId("azure_openai".into())
    }

    fn provider_name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName("Azure OpenAI".into())
    }

    fn telemetry_id(&self) -> String {
        format!("azure_openai/{}", self.id.0)
    }

    fn max_token_count(&self) -> usize {
        200000 // Example value; adjust based on your model's capabilities
    }

    fn count_tokens(
        &self,
        request: LanguageModelRequest,
        cx: &App,
    ) -> BoxFuture<'static, Result<usize>> {
        // Since Azure OpenAI uses OpenAI models, reuse the token counting method
        count_open_ai_tokens(request, open_ai::Model::Four, cx)
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<futures::stream::BoxStream<'static, Result<LanguageModelCompletionEvent>>>,
    > {
        self.stream_completion(request, cx)
    }

    fn use_any_tool(
        &self,
        _request: LanguageModelRequest,
        _name: String,
        _description: String,
        _schema: serde_json::Value,
        _cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<futures::stream::BoxStream<'static, Result<String>>>> {
        // Implement tool usage if needed
        futures::future::ready(Err(anyhow!("not implemented"))).boxed()
    }
}

impl State {
    fn get_credentials(&self) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
        (
            self.api_key.clone(),
            self.api_url.clone(),
            self.deployment_name.clone(),
            self.api_version.clone(),
        )
    }
}

fn into_azure_open_ai_request(
    request: language_model::LanguageModelRequest,
) -> open_ai::Request {
    // Define the default system message.
    let default_system_message = open_ai::RequestMessage::System {
        content: "Formatting re-enabled - please enclose code blocks with appropriate Markdown tags.".to_string(),
    };

    // Start with an empty messages vector.
    let mut messages: Vec<open_ai::RequestMessage> = Vec::new();

    // Check if there's already a system message provided.
    let has_system_message = request.messages.iter().any(|msg| matches!(msg.role, Role::System));
    if !has_system_message {
        // Prepend the default system message if none exists.
        messages.push(default_system_message);
    }

    // Convert and add all the incoming messages.
    messages.extend(request.messages.into_iter().map(|msg| match msg.role {
        Role::User => open_ai::RequestMessage::User {
            content: msg.string_contents(),
        },
        Role::Assistant => open_ai::RequestMessage::Assistant {
            content: Some(msg.string_contents()),
            tool_calls: Vec::new(),
        },
        Role::System => open_ai::RequestMessage::System {
            content: msg.string_contents(),
        },
    }));

    open_ai::Request {
        model: "".to_string(), // The model is selected via deployment_name in Azure.
        messages,
        stream: true,
        stop: request.stop,
        temperature: request.temperature.unwrap_or(1.0),
        max_tokens: None, // You can adjust this based on model capabilities.
        tools: Vec::new(),
        tool_choice: None,
    }
}


async fn stream_azure_open_ai_completion(
    client: &dyn HttpClient,
    api_url: &str,
    api_key: &str,
    deployment_name: &str,
    api_version: &str,
    request_payload: serde_json::Value,
) -> Result<futures::stream::BoxStream<'static, Result<open_ai::ResponseStreamEvent>>> {
    let uri = format!(
        "{}/openai/deployments/{}/chat/completions?api-version={}",
        api_url.trim_end_matches('/'),
        deployment_name,
        api_version
    );
    let request_builder = http_client::Request::builder()
        .method(http_client::Method::POST)
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("api-key", api_key);
    let request_body = serde_json::to_string(&request_payload)?;
    let request = request_builder.body(http_client::AsyncBody::from(request_body))?;
    let mut response = client.send(request).await?;
    if response.status().is_success() {
        let reader = futures::io::BufReader::new(response.into_body());
        Ok(reader
            .lines()
            .filter_map(|line| async move {
                match line {
                    Ok(line) => {
                        let line = line.strip_prefix("data: ")?;
                        if line == "[DONE]" {
                            None
                        } else {
                            match serde_json::from_str::<open_ai::ResponseStreamResult>(line) {
                                Ok(open_ai::ResponseStreamResult::Ok(response)) => Some(Ok(response)),
                                Ok(open_ai::ResponseStreamResult::Err { error }) => Some(Err(anyhow!(error))),
                                Err(error) => Some(Err(anyhow!(error))),
                            }
                        }
                    }
                    Err(error) => Some(Err(anyhow!(error))),
                }
            })
            .boxed())
    } else {
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;
        Err(anyhow!(
            "Error from Azure OpenAI API: {}",
            response.status()
        ))
    }
}

// Use the existing OpenAI method to extract text from events
pub fn extract_text_from_azure_events(
    response: impl Stream<Item = Result<open_ai::ResponseStreamEvent>>,
) -> impl Stream<Item = Result<String>> {
    response.filter_map(|response| async move {
        match response {
            Ok(mut response) => {
                if let Some(choice) = response.choices.pop() {
                    if let Some(content) = choice.delta.content {
                        Some(Ok(content))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            Err(error) => Some(Err(error)),
        }
    })
}

// Implement the Configuration View for Azure OpenAI

struct AzureConfigurationView {
    state: gpui::Entity<State>,
    api_key_editor: Entity<Editor>,
    api_url_editor: Entity<Editor>,
    deployment_name_editor: Entity<Editor>,
    api_version_editor: Entity<Editor>,
}

impl AzureConfigurationView {
    fn new(state: gpui::Entity<State>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let api_key_editor = cx.new(|cx| Editor::single_line(window, cx));
        let api_url_editor = cx.new(|cx| Editor::single_line(window, cx));
        let deployment_name_editor = cx.new(|cx| Editor::single_line(window, cx));
        let api_version_editor = cx.new(|cx| Editor::single_line(window, cx));

        cx.observe(&state, |_, _, cx| {
            cx.notify();
        })
        .detach();

        Self {
            state,
            api_key_editor,
            api_url_editor,
            deployment_name_editor,
            api_version_editor,
        }
    }

    fn save_credentials(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let api_key = self.api_key_editor.read(cx).text(cx);
        let api_url = self.api_url_editor.read(cx).text(cx);
        let deployment_name = self.deployment_name_editor.read(cx).text(cx);
        let api_version = self.api_version_editor.read(cx).text(cx);

        if api_key.is_empty() || api_url.is_empty() || deployment_name.is_empty() || api_version.is_empty() {
            // Handle error: all fields are required
            return;
        }

        let state = self.state.clone();
        cx.spawn_in(window, |_, mut cx| async move {
            state
                .update(&mut cx, |state, cx| {
                    state.set_credentials(
                        api_key.clone(),
                        api_url.clone(),
                        deployment_name.clone(),
                        api_version.clone(),
                        cx,
                    )
                })?
                .await
        })
        .detach_and_log_err(cx);

        cx.notify();
    }

    fn reset_credentials(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.api_key_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));
        self.api_url_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));
        self.deployment_name_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));
        self.api_version_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));

        let state = self.state.clone();
        cx.spawn_in(window, |_, mut cx| async move {
            state
                .update(&mut cx, |state, cx| state.reset_credentials(cx))?
                .await
        })
        .detach_and_log_err(cx);

        cx.notify();
    }

    fn render_editor(
        &self,
        editor: &Entity<Editor>,
        placeholder_text: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let settings = ThemeSettings::get_global(cx);
        let text_style = TextStyle {
            color: cx.theme().colors().text,
            font_family: settings.ui_font.family.clone(),
            font_features: settings.ui_font.features.clone(),
            font_fallbacks: settings.ui_font.fallbacks.clone(),
            font_size: rems(0.875).into(),
            font_weight: settings.ui_font.weight,
            font_style: FontStyle::Normal,
            line_height: relative(1.3),
            white_space: WhiteSpace::Normal,
            ..Default::default()
        };

        let placeholder = placeholder_text.to_string();  // Create owned String
        v_flex()
            .child(
                EditorElement::new(
                    editor,
                    EditorStyle {
                        background: cx.theme().colors().editor_background,
                        local_player: cx.theme().players().local(),
                        text: text_style,
                        ..Default::default()
                    },
                )
            )
            .child(Label::new(placeholder).color(Color::Muted))
    }
}

impl Render for AzureConfigurationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let authenticated = self.state.read(cx).is_authenticated();

        if authenticated {
            h_flex()
                .size_full()
                .justify_between()
                .child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::Check).color(Color::Success))
                        .child(Label::new("Azure OpenAI configured.")),
                )
                .child(
                    Button::new("reset-credentials", "Reset Credentials")
                        .icon(Some(IconName::Trash))
                        .icon_size(IconSize::Small)
                        .icon_position(IconPosition::Start)
                        .on_click(cx.listener(|this, _, window, cx| this.reset_credentials(window, cx))),
                )
                .into_any()
        } else {
            v_flex()
                .size_full()
                .on_action(cx.listener(Self::save_credentials))
                .child(Label::new("Configure Azure OpenAI:"))
                .child(Label::new("API Key:"))
                .child(self.render_editor(&self.api_key_editor, "Enter your API Key", cx))
                .child(Label::new("API URL:"))
                .child(self.render_editor(&self.api_url_editor, "https://<your-resource-name>.openai.azure.com", cx))
                .child(Label::new("Deployment Name:"))
                .child(self.render_editor(&self.deployment_name_editor, "Your deployment name", cx))
                .child(Label::new("API Version:"))
                .child(self.render_editor(&self.api_version_editor, "e.g., 2023-05-15", cx))
                .child(
                    Button::new("save-credentials", "Save")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_credentials(&menu::Confirm, window, cx)
                        })),
                )
                .into_any()
        }
    }
}
