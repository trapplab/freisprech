//! Experimental: corrects a transcript with a small local LLM (see ROADMAP.md).

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use foundry_local_sdk::{
    ChatSession, FoundryLocalManager, Item, Model, Request, RequestOptions, SearchOptions, TextKind,
};

pub const DEFAULT_MODEL: &str = "qwen3-0.6b";

/// Prompt plus dictation plus answer must fit; a few minutes of speech is well below.
const MAX_LENGTH: u64 = 4096;

const INSTRUCTIONS: &str = "You correct text from speech recognition. Fix spelling, grammar, \
punctuation and capitalization. Remove filler words such as um, uh, äh, ähm. When the speaker \
corrects themselves, keep only the correction. Keep the language, wording and meaning. Do not \
translate, summarize, answer questions or add anything. Reply with the corrected text only.";

/// Shown to the model before the text, as earlier turns of the chat.
const EXAMPLES: &[(&str, &str)] = &[
    ("hallo wie geht es dir", "Hallo, wie geht es dir?"),
    (
        "ähm der hund ist braun nein schwarz",
        "Der Hund ist schwarz.",
    ),
    ("uh the sky is blue", "The sky is blue."),
];

pub struct Polisher {
    _manager: Arc<FoundryLocalManager>,
    model: Arc<Model>,
}

impl Polisher {
    /// Downloads the model on first use (reporting progress in percent) and loads it.
    pub async fn load(alias: &str, on_progress: impl FnMut(f64) + Send + 'static) -> Result<Self> {
        let (manager, model) = crate::asr::fetch_model(alias, on_progress).await?;
        limit_max_length(&model.path().await?)?;
        model.load().await?;
        Ok(Self {
            _manager: manager,
            model,
        })
    }

    pub async fn polish(&self, text: &str) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }
        // A new session per text, so earlier texts don't come along as chat history.
        let session = ChatSession::new(&self.model).await?;
        // Qwen3 thinks before answering unless told otherwise; that only costs time here.
        // `enable_thinking` in `chat_template_kwargs` would be cleaner, but Foundry's
        // template engine fails on Qwen3's template then.
        let mut messages = vec![Item::system_message(vec![Item::text(INSTRUCTIONS)])];
        for (input, output) in EXAMPLES {
            messages.push(Item::user_message(vec![Item::text(format!(
                "{input} /no_think"
            ))]));
            messages.push(Item::assistant_message(vec![Item::text(*output)]));
        }
        messages.push(Item::user_message(vec![Item::text(format!(
            "{text} /no_think"
        ))]));
        let request = Request::from_items(messages).with_options(RequestOptions {
            search: SearchOptions {
                do_sample: Some(false),
                max_output_tokens: Some(1024),
                ..Default::default()
            },
            ..Default::default()
        });
        let response = session.process_request(request).await?;
        tracing::debug!(?response, "LLM response");
        Ok(answer(&response.items).trim().to_owned())
    }
}

/// The answer text of a chat response, without the reasoning.
fn answer(items: &[Item]) -> String {
    let mut out = String::new();
    for item in items {
        match item {
            Item::Text {
                text,
                kind: TextKind::Default,
            } => out.push_str(text),
            Item::Message(message) => out.push_str(&answer(&message.content)),
            _ => {}
        }
    }
    out
}

/// The engine allocates the KV cache for `search.max_length` on every request: 40960 tokens
/// for Qwen3 take about 6 s on a laptop CPU. There is no request option for it, so the
/// downloaded model config is changed.
fn limit_max_length(model_dir: &Path) -> Result<()> {
    let path = model_dir.join("genai_config.json");
    let text = std::fs::read(&path).with_context(|| format!("Cannot read {}", path.display()))?;
    let mut config: serde_json::Value = serde_json::from_slice(&text)?;
    let max_length = &mut config["search"]["max_length"];
    if max_length.as_u64().is_some_and(|n| n > MAX_LENGTH) {
        *max_length = MAX_LENGTH.into();
        std::fs::write(&path, serde_json::to_vec_pretty(&config)?)
            .with_context(|| format!("Cannot write {}", path.display()))?;
        tracing::info!(path = %path.display(), MAX_LENGTH, "Limited max_length");
    }
    Ok(())
}
