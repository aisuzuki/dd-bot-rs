//! DeepL translatior bot for Discord
//!
use std::{
    collections::HashSet,
    env,
    error::Error,
    fmt::{Display, Formatter},
};

use serenity::{
    async_trait, http::Http, model::event::ResumedEvent, model::gateway::Ready, prelude::*,
};

use tracing::{error, info, warn};
use url::Url;

//use crate::commands::meta::*;
//use crate::commands::owner::*;

#[derive(serde::Deserialize, Debug)]
struct DdBotChannelConfig {
    pub default_lang: String,
    pub target_lang: String,
}

mod deepl {
    #[derive(serde::Serialize, Debug)]
    pub(crate) struct DeepLTranslationRequestBody {
        pub text: Vec<String>,
        pub target_lang: String,
    }

    #[derive(serde::Deserialize, Debug)]
    pub(crate) struct DeeplTranslationResopnse {
        pub translations: Vec<DeepLTranslation>,
    }

    #[derive(serde::Deserialize, Debug)]
    pub(crate) struct DeepLTranslation {
        pub text: String,
        pub detected_source_language: String,
    }
}

impl Display for deepl::DeeplTranslationResopnse {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let formatted_message = self
            .translations
            .iter()
            .map(|t| format!("`{}:` {}", t.detected_source_language, t.text))
            .collect::<Vec<String>>()
            .join("\n");
        write!(f, "{}", formatted_message)
    }
}

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

// Event handler for the bot
struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        info!("Connected as {}", ready.user.name);
    }

    async fn resume(&self, _: Context, _: ResumedEvent) {
        info!("Resumed");
    }

    async fn message(&self, ctx: Context, msg: serenity::model::channel::Message) {
        // Ignore messages from bots and empty messages (including image posts)
        if msg.author.bot || msg.content.is_empty() {
            return;
        }
        if Url::parse(msg.content.as_str()).is_ok() {
            // do not need to traslate links
            return;
        }

        let config = get_language_config(&ctx, &msg).await;
        let deepl_api_key = &env::var("DEEPL_API_KEY").unwrap(); // Safe unwrap - the value is checked at startup

        match deepl_translate(&msg.content, &config.target_lang, deepl_api_key) {
            Ok(translation_result) => {
                if let Some(reply_message) =
                    create_reply_message(&translation_result, &config, &msg.content, deepl_api_key)
                {
                    if let Err(reason) = msg.reply(&ctx.http, reply_message).await {
                        error!("Failed to reply translation result: {:?}", reason);
                    }
                } else {
                    // currently the translation will be ignored in unexpected cases, which is multiple translation results.
                    warn!(
                        "Unexpected translation result: {:?} - ignoring",
                        translation_result
                    );
                }
            }
            Err(e) => {
                error!("Error translating message: {:?}", e);
                if let Err(reason) = msg
                    .reply(&ctx.http, "Failed to translate using DeepL")
                    .await
                {
                    error!("Failed to reply error message:  {:?}", reason);
                }
            }
        }
    }
}

// Create a reply message using the translation result from DeepL API
// translate from `detected_source_language` to `target_language`.
//
// If the detected source language is the same as the target language, (e.g, `JA => JA`)
// then translate the original text to the default language (e.g, `JA => EN`)
//
// Also, if the detected source language is unknown language to the channel (e.g, `NL`),
// then translate the original text to both the target language and the default language.
//
fn create_reply_message(
    deepl_response: &deepl::DeeplTranslationResopnse,
    language_config: &DdBotChannelConfig,
    original_text: &str,
    deepl_api_key: &str,
) -> Option<String> {
    // one to one translation is done
    if deepl_response.translations.len() > 1 {
        warn!(
            "Received multiple translations from DeepL API: {:?}",
            deepl_response
        );
        return None;
    }

    // expect one translation since the request only contains single text
    let translation = &deepl_response.translations[0];

    if translation.detected_source_language != language_config.target_lang
        && translation.detected_source_language != language_config.default_lang
    {
        // if the detected source language was not `default_language` <=> `target_language` translation,
        // e.g, `NL`: detected source language, `JA`: target language, `EN`: default language
        // add default language translation, too
        if let Ok(translation_result) =
            deepl_translate(original_text, &language_config.default_lang, deepl_api_key)
        {
            Some(format!(
                "`{}: ` {} \n`{}: ` {} \n(translated from `{}`)",
                language_config.target_lang,
                translation.text,
                language_config.default_lang,
                translation_result.translations[0].text, // expect translation result to be true
                translation.detected_source_language
            ))
        } else {
            // failed to execute reverse translation
            Some(format!(
                "Failed to translate to `{}`",
                language_config.default_lang
            ))
        }
    } else if translation.detected_source_language == language_config.target_lang {
        // if the detected source language is the same as the target language,
        // return reverse translation (e.g, `JA(target_language)` => EN(default language)`)
        if let Ok(translation_result) =
            deepl_translate(original_text, &language_config.default_lang, deepl_api_key)
        {
            Some(format!(
                "`{}: ` {}",
                language_config.default_lang,
                translation_result.translations[0].text, // expect translation result to be true
            ))
        } else {
            // failed to execute reverse translation
            Some(format!(
                "Failed to translate `{}` to `{}`",
                original_text, language_config.default_lang
            ))
        }
    } else {
        // normal case - other language to target language (e.g, `EN`(default_language) => `JP`(target_language))
        Some(format!(
            "`{}: {}`",
            language_config.target_lang, translation.text
        ))
    }
}

// Get language configuration from the channel topic
async fn get_language_config(
    ctx: &Context,
    msg: &serenity::model::channel::Message,
) -> DdBotChannelConfig {
    // Default language configuration from environment variables
    let default_lang = &env::var("DEFAULT_LANGUAGE").unwrap_or(String::from("JA"));
    let target_lang = &env::var("TARGET_LANGUAGE").unwrap_or(String::from("JA"));
    let default_config = DdBotChannelConfig {
        default_lang: default_lang.clone(),
        target_lang: target_lang.clone(),
    };

    let Ok(channel) = msg.channel(&ctx.http).await else {
        error!("Failed to get channel for message - use default {default_lang}");
        return default_config;
    };
    let Some(guild_channel) = channel.guild() else {
        error!("Failed to get channel for message - use default {default_lang}");
        return default_config;
    };

    let Some(topic) = guild_channel.topic else {
        error!("Failed to get channel for message - use default {target_lang}");
        return default_config;
    };

    let config: DdBotChannelConfig = serde_json::from_str(&topic).unwrap_or(default_config);
    config
}

// Send a translation request to the DeepL API
fn deepl_translate(
    text: &str,
    target_lang: &str,
    api_key: &str,
) -> Result<deepl::DeeplTranslationResopnse> {
    match ureq::post("https://api.deepl.com/v2/translate")
        .header("Authorization", format!("DeepL-Auth-Key {api_key}"))
        .header("Content-Type", "application/json")
        .send_json(deepl::DeepLTranslationRequestBody {
            text: vec![text.to_string()],
            target_lang: target_lang.to_string(),
        }) {
        Ok(mut response) => {
            let translated_texts = response
                .body_mut()
                .read_json::<deepl::DeeplTranslationResopnse>()?;

            Ok(translated_texts)
        }
        Err(ureq::Error::StatusCode(code)) => Err(format!("Server error: {code}").into()),
        Err(_) => Err("Failed to connect to DeepL API".into()),
    }
}

#[tokio::main]
async fn main() {
    // Read `.env` for Discord token and DeepL API key.
    dotenv::dotenv().expect("Failed to load .env file");
    // check for required environment variables
    let _deepl_auth_key =
        &env::var("DEEPL_API_KEY").expect("Expected an api key in the environment");
    let token = &env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    // Initialize the logger to use environment variables.
    //
    // In this case, a good default is setting the environment variable `RUST_LOG` to `debug`.
    tracing_subscriber::fmt::init();

    // TODO: In order to be able to use commands, we need to register them once.
    // https://discord.com/developers/docs/interactions/application-commands#registering-a-command
    // For now, commands are not necessary for this bot.

    // Fetch application info
    let http = Http::new(token);
    let (_owners, _bot_id) = match http.get_current_application_info().await {
        Ok(info) => {
            let mut owners = HashSet::new();
            if let Some(owner) = &info.owner {
                owners.insert(owner.id);
            }

            (owners, info.id)
        }
        Err(why) => panic!("Could not access application info: {:?}", why),
    };

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(token, intents)
        .event_handler(Handler)
        .await
        .expect("Err creating client");

    let shard_manager = client.shard_manager.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Could not register ctrl+c handler");
        shard_manager.shutdown_all().await;
    });

    if let Err(why) = client.start().await {
        error!("Client error: {:?}", why);
    }
}
