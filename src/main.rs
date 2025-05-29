//! DeepL translatior bot for Discord
//!
use std::{collections::HashSet, env, error::Error, fmt::Display, fmt::Formatter};

use serenity::{
    async_trait, http::Http, model::event::ResumedEvent, model::gateway::Ready, prelude::*,
};

use tracing::{error, info};
use url::Url;

//use crate::commands::meta::*;
//use crate::commands::owner::*;

#[derive(serde::Deserialize, Debug)]
struct DdBotConfig {
    pub target_lang: String,
}

#[derive(serde::Serialize, Debug)]
struct DeepLTranslationRequestBody {
    pub text: Vec<String>,
    pub target_lang: String,
}

#[derive(serde::Deserialize, Debug)]
struct DeeplTranslationResopnse {
    pub translations: Vec<DeepLTranslation>,
}

#[derive(serde::Deserialize, Debug)]
struct DeepLTranslation {
    pub text: String,
    pub detected_source_language: String,
}

impl Display for DeeplTranslationResopnse {
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

        let target_lang = get_target_language(&ctx, &msg).await;
        let deepl_api_key =
            &env::var("DEEPL_API_KEY").expect("Expected an api key in the environment");

        if let Ok(translation_result) = deepl_translate(&msg.content, &target_lang, deepl_api_key) {
            // TODO: if the detected source language was not target language,

            if let Err(reason) = msg.reply(&ctx.http, translation_result.to_string()).await {
                error!("Failed to send message: {:?}", reason);
            }
        } else {
            // TODO: log
            error!("Failed to translate message");
        }
    }
}

async fn get_target_language(ctx: &Context, msg: &serenity::model::channel::Message) -> String {
    if let Ok(channel) = msg.channel(&ctx.http).await {
        if let Some(guild_channel) = channel.guild() {
            if let Some(topic) = guild_channel.topic {
                let config: DdBotConfig = serde_json::from_str(&topic).unwrap_or(DdBotConfig {
                    target_lang: "JA".to_string(), // Default language
                });
                return config.target_lang.to_ascii_uppercase();
            }
        }
    }
    env::var("DEFAULT_LANGUAGE").unwrap_or(String::from("JA"))
}

fn deepl_translate(
    text: &str,
    target_lang: &str,
    api_key: &str,
) -> Result<DeeplTranslationResopnse> {
    match ureq::post("https://api.deepl.com/v2/translate")
        .header("Authorization", format!("DeepL-Auth-Key {api_key}"))
        .header("Content-Type", "application/json")
        .send_json(DeepLTranslationRequestBody {
            text: vec![text.to_string()],
            target_lang: target_lang.to_string(),
        }) {
        Ok(mut response) => {
            let translated_texts = response
                .body_mut()
                .read_json::<DeeplTranslationResopnse>()?;

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
        env::var("DEEPL_API_KEY").expect("Expected an api key in the environment");
    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    // Initialize the logger to use environment variables.
    //
    // In this case, a good default is setting the environment variable `RUST_LOG` to `debug`.
    tracing_subscriber::fmt::init();

    // TODO: In order to be able to use commands, we need to register them once.
    // https://discord.com/developers/docs/interactions/application-commands#registering-a-command
    // For now, commands are not necessary for this bot.

    // Fetch application info
    let http = Http::new(&token);
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
    let mut client = Client::builder(&token, intents)
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
