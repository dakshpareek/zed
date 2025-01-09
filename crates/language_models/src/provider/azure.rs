use super::open_ai::count_open_ai_tokens;
use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt, Stream, StreamExt};
use gpui::{
    prelude::*, AnyView, AppContext, AsyncAppContext, FontStyle, Model, ModelContext, Subscription,
    Task, TextStyle, View, WhiteSpace,
};
use http_client::{AsyncBody, HttpClient, Method, Request as HttpRequest};
use language_model::{
    LanguageModel, LanguageModelCompletionEvent, LanguageModelId, LanguageModelName,
    LanguageModelProvider, LanguageModelProviderId, LanguageModelProviderName,
    LanguageModelProviderState, LanguageModelRequest, RateLimiter,
};
use open_ai::{self, Request as OpenAiRequest, ResponseStreamEvent, ResponseStreamResult};
use settings::{Settings, SettingsStore};
use std::sync::Arc;
use strum::IntoEnumIterator;
use theme::ThemeSettings;
use ui::{prelude::*, Icon, IconName, Tooltip};

use editor::{Editor, EditorElement, EditorStyle};
use fs::Fs; // Import the Fs trait
use futures::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use log::info;
use util::ResultExt;

use crate::AllLanguageModelSettings;

pub const PROVIDER_ID: &str = "azure_openai";
pub const PROVIDER_NAME: &str = "Azure OpenAI";

pub struct AzureLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: Model<State>,
}

pub struct State {
    api_key: Option<String>,
    api_key_from_env: bool,
    fs: Arc<dyn Fs>, // Add fs to State
    _subscription: Subscription,
}

impl AzureLanguageModelProvider {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        fs: Arc<dyn Fs>,
        cx: &mut AppContext,
    ) -> Self {
        let state = cx.new_model(|cx| State {
            api_key: None,
            api_key_from_env: false,
            fs: fs.clone(), // Store fs in the state
            _subscription: cx.observe_global::<SettingsStore>(|_this, cx| {
                cx.notify();
            }),
        });

        Self { http_client, state }
    }
}

impl LanguageModelProviderState for AzureLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<Model<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for AzureLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        LanguageModelProviderId(PROVIDER_ID.to_string().into())
    }

    fn name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName(PROVIDER_NAME.to_string().into())
    }

    fn icon(&self) -> IconName {
        // Reuse OpenAI icon or use Azure icon if available
        IconName::AiOpenAi
    }

    fn provided_models(&self, _cx: &AppContext) -> Vec<Arc<dyn LanguageModel>> {
        // Reuse OpenAI models
        open_ai::Model::iter()
            .map(|model| {
                Arc::new(AzureOpenAiLanguageModel {
                    id: LanguageModelId::from(model.id().to_string()),
                    model,
                    state: self.state.clone(),
                    http_client: self.http_client.clone(),
                    request_limiter: RateLimiter::new(4),
                }) as Arc<dyn LanguageModel>
            })
            .collect()
    }

    fn is_authenticated(&self, cx: &AppContext) -> bool {
        let authenticated = self.state.read(cx).is_authenticated();
        info!("Azure provider is authenticated: {}", authenticated);
        authenticated
    }

    fn authenticate(&self, cx: &mut AppContext) -> Task<Result<()>> {
        self.state.update(cx, |state, cx| state.authenticate(cx))
    }

    fn configuration_view(&self, cx: &mut WindowContext) -> AnyView {
        cx.new_view(|cx| ConfigurationView::new(self.state.clone(), cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut AppContext) -> Task<Result<()>> {
        self.state.update(cx, |state, cx| state.reset_api_key(cx))
    }
}

pub struct AzureOpenAiLanguageModel {
    id: LanguageModelId,
    model: open_ai::Model,
    state: Model<State>,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl LanguageModel for AzureOpenAiLanguageModel {
    fn id(&self) -> LanguageModelId {
        self.id.clone()
    }

    fn name(&self) -> LanguageModelName {
        LanguageModelName::from(self.model.display_name().to_string())
    }

    fn provider_id(&self) -> LanguageModelProviderId {
        LanguageModelProviderId(PROVIDER_ID.to_string().into())
    }

    fn provider_name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName(PROVIDER_NAME.to_string().into())
    }

    fn telemetry_id(&self) -> String {
        format!("azure/{}", self.model.id())
    }

    fn max_token_count(&self) -> usize {
        self.model.max_token_count()
    }

    fn count_tokens(
        &self,
        request: LanguageModelRequest,
        cx: &AppContext,
    ) -> BoxFuture<'static, Result<usize>> {
        count_open_ai_tokens(request, self.model.clone(), cx)
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        cx: &AsyncAppContext,
    ) -> BoxFuture<'static, Result<BoxStream<'static, Result<LanguageModelCompletionEvent>>>> {
        let model_deployment_name = self.model.id().to_string();
        let api_version = "2024-08-01-preview"; // Adjust API version as needed
        let request = request.into_open_ai(model_deployment_name.clone(), self.max_output_tokens());
        let completions =
            self.stream_completion(request, model_deployment_name, api_version, cx);
        async move {
            Ok(open_ai::extract_text_from_events(completions.await?)
                .map(|result| result.map(LanguageModelCompletionEvent::Text))
                .boxed())
        }
        .boxed()
    }

    fn use_any_tool(
        &self,
        _request: LanguageModelRequest,
        _tool_name: String,
        _tool_description: String,
        _schema: serde_json::Value,
        _cx: &AsyncAppContext,
    ) -> BoxFuture<'static, Result<BoxStream<'static, Result<String>>>> {
        // Implement tool usage if needed
        todo!()
    }
}

impl AzureOpenAiLanguageModel {
    fn stream_completion(
        &self,
        request: OpenAiRequest,
        deployment_name: String,
        api_version: &'static str,
        cx: &AsyncAppContext,
    ) -> BoxFuture<
        'static,
        Result<futures::stream::BoxStream<'static, Result<ResponseStreamEvent>>>,
    > {
        let http_client = self.http_client.clone();
        let state = self.state.clone();

        let (api_key, settings) = match cx.read_model(&state, |state, cx| {
            let settings = AllLanguageModelSettings::get_global(cx).azure.clone();
            (state.api_key.clone(), settings)
        }) {
            Ok((Some(api_key), settings)) => (api_key, settings),
            Ok((None, _)) => return futures::future::ready(Err(anyhow!("Missing Azure OpenAI API Key"))).boxed(),
            Err(err) => return futures::future::ready(Err(anyhow!("App state error: {}", err))).boxed(),
        };

        let api_url = settings.api_url.unwrap_or_default();

        // Use the deployment name and API version from settings if available
        let deployment_name = settings.deployment_name.unwrap_or(deployment_name);
        let api_version = settings.api_version.unwrap_or_else(|| api_version.to_string());

        let future = self.request_limiter.stream(async move {
            let is_azure_endpoint = api_url.contains(".azure.com");
            let api_url = if is_azure_endpoint {
                format!(
                    "{}/openai/deployments/{}/chat/completions?api-version={}",
                    api_url.trim_end_matches('/'),
                    deployment_name,
                    api_version
                )
            } else {
                // If not an Azure endpoint, use the standard OpenAI API URL
                format!("{}/v1/chat/completions", api_url.trim_end_matches('/'))
            };

            let response_stream =
                azure_stream_completion(http_client.as_ref(), &api_url, &api_key, request).await?;
            Ok(response_stream)
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }
}

async fn azure_stream_completion(
    client: &dyn HttpClient,
    api_url: &str,
    api_key: &str,
    request: OpenAiRequest,
) -> Result<impl Stream<Item = Result<ResponseStreamEvent>> + Send + 'static> {
    let is_azure_endpoint = api_url.contains(".azure.com");
    let (auth_header_name, auth_value) = if is_azure_endpoint {
        ("api-key", api_key.to_string())
    } else {
        ("Authorization", format!("Bearer {}", api_key))
    };

    let request_builder = HttpRequest::builder()
        .method(Method::POST)
        .uri(api_url)
        .header("Content-Type", "application/json")
        .header(auth_header_name, auth_value);

    // Log headers before consuming the request_builder
    let headers = request_builder.headers_ref().cloned(); // Clone the headers for logging

    let request_body = serde_json::to_string(&request)?;
    let request = request_builder.body(AsyncBody::from(request_body.clone()))?;

    let mut response = client.send(request).await?;
    if response.status().is_success() {
        let reader = BufReader::new(response.into_body());
        Ok(reader
            .lines()
            .filter_map(|line_result| async move {
                let line = match line_result {
                    Ok(line) => line,
                    Err(error) => return Some(Err(anyhow!(error))),
                };
                let line = line.trim();
                if line.is_empty() {
                    return None;
                }

                let line = line.strip_prefix("data: ")?;
                if line == "[DONE]" {
                    None
                } else {
                    match serde_json::from_str::<ResponseStreamResult>(line) {
                        Ok(ResponseStreamResult::Ok(response)) => Some(Ok(response)),
                        Ok(ResponseStreamResult::Err { error }) => Some(Err(anyhow!(error))),
                        Err(error) => Some(Err(anyhow!(error))),
                    }
                }
            })
            .boxed())
    } else {
        // Log the request details when the request fails
        let mut response_body = String::new();
        response.body_mut().read_to_string(&mut response_body).await?;
        log::error!(
            "Azure OpenAI API request failed:\n\
            URL: {}\n\
            Headers: {:?}\n\
            Body: {}\n\
            Status: {}\n\
            Response Body: {}",
            api_url,
            headers.unwrap_or_default(), // Use the cloned headers here
            request_body,
            response.status(),
            response_body
        );

        Err(anyhow!(
            "Error from Azure OpenAI API: {}",
            response.status()
        ))
    }
}

// Implement State methods
impl State {
    fn is_authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    fn authenticate(&self, cx: &mut ModelContext<Self>) -> Task<Result<()>> {
        if self.is_authenticated() {
            Task::ready(Ok(()))
        } else {
            let api_url = AllLanguageModelSettings::get_global(cx)
                .azure
                .api_url
                .clone()
                .unwrap_or_default();

            cx.spawn(|this, mut cx| async move {
                let (api_key, from_env) =
                    if let Ok(api_key) = std::env::var("AZURE_OPENAI_API_KEY") {
                        (api_key, true)
                    } else {
                        let (_, api_key) = cx
                            .update(|cx| cx.read_credentials(&api_url))?
                            .await?
                            .ok_or_else(|| anyhow!("Credentials not found"))?;
                        (String::from_utf8(api_key)?, false)
                    };
                this.update(&mut cx, |this, cx| {
                    this.api_key = Some(api_key);
                    this.api_key_from_env = from_env;
                    cx.notify();
                })
            })
        }
    }

    fn reset_api_key(&self, cx: &mut ModelContext<Self>) -> Task<Result<()>> {
        let api_url = AllLanguageModelSettings::get_global(cx)
            .azure
            .api_url
            .clone()
            .unwrap_or_default();

        let delete_credentials = cx.delete_credentials(&api_url);
        cx.spawn(|this, mut cx| async move {
            delete_credentials.await.log_err();
            this.update(&mut cx, |this, cx| {
                this.api_key = None;
                this.api_key_from_env = false;
                cx.notify();
            })
        })
    }

    fn set_api_key_and_endpoint(
        &mut self,
        api_key: String,
        endpoint: String,
        deployment: String,
        api_version: String,
        cx: &mut ModelContext<Self>,
    ) -> Task<Result<()>> {
        // Clone the endpoint before moving it into the closure
        let endpoint_for_settings = endpoint.clone();
        let deployment_for_settings = deployment.clone();
        let api_version_for_settings = api_version.clone();

        // Save the endpoint to settings
        let fs = self.fs.clone();

        settings::update_settings_file::<AllLanguageModelSettings>(
            fs,
            cx,
            move |settings, _| {
                if settings.azure.is_none() {
                    settings.azure = Some(crate::AzureSettingsContent {
                        api_url: Some(endpoint_for_settings.clone()),
                        deployment_name: Some(deployment_for_settings.clone()),
                        api_version: Some(api_version_for_settings.clone()),
                    });
                } else {
                    let azure = settings.azure.as_mut().unwrap();
                    azure.api_url = Some(endpoint_for_settings.clone());
                    azure.deployment_name = Some(deployment_for_settings.clone());
                    azure.api_version = Some(api_version_for_settings.clone());
                }
            },
        );

        // Now you can use `endpoint` here because it wasn't moved
        let write_credentials = cx.write_credentials(&endpoint, "Bearer", api_key.as_bytes());

        cx.spawn(|this, mut cx| async move {
            write_credentials.await?;
            this.update(&mut cx, |this, cx| {
                this.api_key = Some(api_key);
                cx.notify();
            })
        })
    }
}

// Define ConfigurationView
struct ConfigurationView {
    api_key_editor: View<Editor>,
    endpoint_editor: View<Editor>,
    deployment_editor: View<Editor>,
    api_version_editor: View<Editor>,
    state: Model<State>,
    load_credentials_task: Option<Task<()>>,
}

impl ConfigurationView {
    fn new(state: Model<State>, cx: &mut ViewContext<Self>) -> Self {
        let api_key_editor = cx.new_view(|cx| {
            let mut editor = Editor::single_line(cx);
            editor.set_placeholder_text("Enter your Azure OpenAI API key", cx);
            editor
        });

        let endpoint_editor = cx.new_view(|cx| {
            let mut editor = Editor::single_line(cx);
            editor.set_placeholder_text("https://<your-azure-openai-endpoint>", cx);
            editor
        });

        let deployment_editor = cx.new_view(|cx| {
            let mut editor = Editor::single_line(cx);
            editor.set_placeholder_text("Enter your deployment name", cx);
            editor
        });

        let api_version_editor = cx.new_view(|cx| {
            let mut editor = Editor::single_line(cx);
            editor.set_placeholder_text("API version (e.g., 2024-02-15-preview)", cx);
            editor
        });

        cx.observe(&state, |_, _, cx| {
            cx.notify();
        })
        .detach();

        let load_credentials_task = Some(cx.spawn({
            let state = state.clone();
            |this, mut cx| async move {
                if let Some(task) = state
                    .update(&mut cx, |state, cx| state.authenticate(cx))
                    .log_err()
                {
                    let _ = task.await;
                }

                this.update(&mut cx, |this, cx| {
                    this.load_credentials_task = None;
                    cx.notify();
                })
                .log_err();
            }
        }));

        Self {
            api_key_editor,
            endpoint_editor,
            deployment_editor,
            api_version_editor,
            state,
            load_credentials_task,
        }
    }

    fn save_credentials(&mut self, _: &menu::Confirm, cx: &mut ViewContext<Self>) {
        let api_key = self.api_key_editor.read(cx).text(cx);
        let endpoint = self.endpoint_editor.read(cx).text(cx);
        let deployment = self.deployment_editor.read(cx).text(cx);
        let api_version = self.api_version_editor.read(cx).text(cx);
        if api_key.is_empty() || endpoint.is_empty() || deployment.is_empty() || api_version.is_empty() {
            return;
        }

        let state = self.state.clone();
        cx.spawn(|_, mut cx| async move {
            state
                .update(&mut cx, |state, cx| {
                    state.set_api_key_and_endpoint(api_key, endpoint,deployment, api_version, cx)
                })?
                .await
        })
        .detach_and_log_err(cx);

        cx.notify();
    }

    fn reset_credentials(&mut self, cx: &mut ViewContext<Self>) {
        self.api_key_editor
            .update(cx, |editor, cx| editor.set_text("", cx));
        self.endpoint_editor
            .update(cx, |editor, cx| editor.set_text("", cx));

        let state = self.state.clone();
        cx.spawn(|_, mut cx| async move {
            state
                .update(&mut cx, |state, cx| state.reset_api_key(cx))?
                .await
        })
        .detach_and_log_err(cx);

        cx.notify();
    }

    fn render_api_key_editor(&self, cx: &mut ViewContext<Self>) -> impl IntoElement {
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
            background_color: None,
            underline: None,
            strikethrough: None,
            white_space: WhiteSpace::Normal,
            truncate: None,
        };
        EditorElement::new(
            &self.api_key_editor,
            EditorStyle {
                background: cx.theme().colors().editor_background,
                local_player: cx.theme().players().local(),
                text: text_style.clone(),
                ..Default::default()
            },
        )
    }

    fn render_endpoint_editor(&self, cx: &mut ViewContext<Self>) -> impl IntoElement {
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
            background_color: None,
            underline: None,
            strikethrough: None,
            white_space: WhiteSpace::Normal,
            truncate: None,
        };
        EditorElement::new(
            &self.endpoint_editor,
            EditorStyle {
                background: cx.theme().colors().editor_background,
                local_player: cx.theme().players().local(),
                text: text_style,
                ..Default::default()
            },
        )
    }

    fn render_deployment_editor(&self, cx: &mut ViewContext<Self>) -> impl IntoElement {
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
            background_color: None,
            underline: None,
            strikethrough: None,
            white_space: WhiteSpace::Normal,
            truncate: None,
        };
        EditorElement::new(
            &self.deployment_editor,
            EditorStyle {
                background: cx.theme().colors().editor_background,
                local_player: cx.theme().players().local(),
                text: text_style.clone(),
                ..Default::default()
            },
        )
    }

    fn render_api_version_editor(&self, cx: &mut ViewContext<Self>) -> impl IntoElement {
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
            background_color: None,
            underline: None,
            strikethrough: None,
            white_space: WhiteSpace::Normal,
            truncate: None,
        };
        EditorElement::new(
            &self.api_version_editor,
            EditorStyle {
                background: cx.theme().colors().editor_background,
                local_player: cx.theme().players().local(),
                text: text_style,
                ..Default::default()
            },
        )
    }

    fn should_render_editor(&self, cx: &mut ViewContext<Self>) -> bool {
        !self.state.read(cx).is_authenticated()
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        const INSTRUCTIONS: [&str; 2] = [
            "To use Azure OpenAI with our assistant, you need to add your API key and endpoint URL. Follow these steps:",
            " - Paste your Azure OpenAI API key and endpoint below, then hit enter to start using the assistant",
        ];

        let env_var_set = self.state.read(cx).api_key_from_env;

        if self.load_credentials_task.is_some() {
            div().child(Label::new("Loading credentials...")).into_any()
        } else if self.should_render_editor(cx) {
            v_flex()
                .size_full()
                .on_action(cx.listener(Self::save_credentials))
                .child(Label::new(INSTRUCTIONS[0]))
                .child(Label::new(INSTRUCTIONS[1]))
                .child(
                    h_flex()
                        .w_full()
                        .my_2()
                        .px_2()
                        .py_1()
                        .bg(cx.theme().colors().editor_background)
                        .rounded_md()
                        .child(Label::new("API Key:"))
                        .child(self.render_api_key_editor(cx)),
                )
                .child(
                    h_flex()
                        .w_full()
                        .my_2()
                        .px_2()
                        .py_1()
                        .bg(cx.theme().colors().editor_background)
                        .rounded_md()
                        .child(Label::new("Endpoint URL:"))
                        .child(self.render_endpoint_editor(cx)),
                )
                .child(
                    h_flex()
                        .w_full()
                        .my_2()
                        .px_2()
                        .py_1()
                        .bg(cx.theme().colors().editor_background)
                        .rounded_md()
                        .child(Label::new("Deployment Name:"))
                        .child(self.render_deployment_editor(cx)),
                )
                .child(
                    h_flex()
                        .w_full()
                        .my_2()
                        .px_2()
                        .py_1()
                        .bg(cx.theme().colors().editor_background)
                        .rounded_md()
                        .child(Label::new("API Version:"))
                        .child(self.render_api_version_editor(cx)),
                )
                .child(
                    Label::new(
                        "Note that having a subscription for another service like GitHub Copilot won't work."
                            .to_string(),
                    )
                    .size(LabelSize::Small),
                )
                .into_any()
        } else {
            h_flex()
                .size_full()
                .justify_between()
                .child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::Check).color(Color::Success))
                        .child(Label::new(if env_var_set {
                            "API key set via environment variable.".to_string()
                        } else {
                            "API key configured.".to_string()
                        })),
                )
                .child(
                    Button::new("reset-key", "Reset key")
                        .icon(Some(IconName::Trash))
                        .icon_size(IconSize::Small)
                        .icon_position(IconPosition::Start)
                        .disabled(env_var_set)
                        .when(env_var_set, |this| {
                            this.tooltip(|cx| {
                                Tooltip::text(
                                    "To reset your API key, unset the AZURE_OPENAI_API_KEY environment variable.",
                                    cx,
                                )
                            })
                        })
                        .on_click(cx.listener(|this, _, cx| this.reset_credentials(cx))),
                )
                .into_any()
        }
    }
}
