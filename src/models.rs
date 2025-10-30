use aws_sdk_dynamodb::types::AttributeValue;
use serde::{Deserialize, Serialize};
use serde_dynamo::aws_sdk_dynamodb_1::{from_item, to_item};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RssRegistration {
    #[serde(rename = "PK")]
    pub pk: String,
    #[serde(rename = "SK")]
    pub sk: String,
    #[serde(rename = "GSI1PK", skip_serializing_if = "Option::is_none")]
    pub gsi1pk: Option<String>,
    #[serde(rename = "GSI1SK", skip_serializing_if = "Option::is_none")]
    pub gsi1sk: Option<String>,
    pub channel_id: u64,
    pub feed_url: String,
    pub last_post_guid: Option<String>,
    pub item_type: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TikTokRegistration {
    #[serde(rename = "PK")]
    pub pk: String,
    #[serde(rename = "SK")]
    pub sk: String,
    #[serde(rename = "GSI1PK", skip_serializing_if = "Option::is_none")]
    pub gsi1pk: Option<String>,
    #[serde(rename = "GSI1SK", skip_serializing_if = "Option::is_none")]
    pub gsi1sk: Option<String>,
    pub item_type: String,
    pub tiktok_open_id: String,
    pub tiktok_display_name: String,
    pub discord_channel_id: u64,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TwitchRegistration {
    #[serde(rename = "PK")]
    pub pk: String,
    #[serde(rename = "SK")]
    pub sk: String,
    #[serde(rename = "GSI1PK", skip_serializing_if = "Option::is_none")]
    pub gsi1pk: Option<String>,
    #[serde(rename = "GSI1SK", skip_serializing_if = "Option::is_none")]
    pub gsi1sk: Option<String>,
    pub twitch_user_id: String,
    pub twitch_login: String,
    pub discord_channel_id: u64,
    pub access_token: String,
    pub refresh_token: String,
    pub item_type: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlueskyRegistration {
    #[serde(rename = "PK")]
    pub pk: String,
    #[serde(rename = "SK")]
    pub sk: String,
    #[serde(rename = "GSI1PK", skip_serializing_if = "Option::is_none")]
    pub gsi1pk: Option<String>,
    #[serde(rename = "GSI1SK", skip_serializing_if = "Option::is_none")]
    pub gsi1sk: Option<String>,
    pub bsky_did: String,
    pub bsky_handle: String,
    pub discord_channel_id: u64,
    pub access_token: String,
    pub refresh_token: String,
    pub last_post_uri: Option<String>,
    pub item_type: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ServerConfig {
    #[serde(rename = "PK")]
    pub pk: String,
    #[serde(rename = "SK")]
    pub sk: String,
    #[serde(rename = "GSI1PK", skip_serializing_if = "Option::is_none")]
    pub gsi1pk: Option<String>,
    #[serde(rename = "GSI1SK", skip_serializing_if = "Option::is_none")]
    pub gsi1sk: Option<String>,
    pub notification_channel_id: u64,
    pub item_type: String,
}

fn table_name() -> String {
    "promote-me".to_string()
}

pub async fn get_all_rss_registrations(
    db: &aws_sdk_dynamodb::Client,
) -> anyhow::Result<Vec<RssRegistration>> {
    let output = db
        .scan()
        .table_name(table_name())
        .filter_expression("item_type = :rss")
        .expression_attribute_values(":rss", AttributeValue::S("RSS_FEED".to_string()))
        .send()
        .await?;

    let items = output.items.unwrap_or_default();
    let regs = items
        .into_iter()
        .filter_map(|item| from_item(item).ok())
        .collect();
    Ok(regs)
}

pub async fn get_all_bluesky_registrations(
    db: &aws_sdk_dynamodb::Client,
) -> anyhow::Result<Vec<BlueskyRegistration>> {
    let output = db
        .scan()
        .table_name(table_name())
        .filter_expression("item_type = :bsky")
        .expression_attribute_values(":bsky", AttributeValue::S("BSKY_REG".to_string()))
        .send()
        .await?;

    let items = output.items.unwrap_or_default();
    let regs = items
        .into_iter()
        .filter_map(|item| from_item(item).ok())
        .collect();
    Ok(regs)
}

pub async fn get_bluesky_reg_by_did(
    db: &aws_sdk_dynamodb::Client,
    bsky_did: &str,
) -> anyhow::Result<Option<BlueskyRegistration>> {
    let current_table_name = table_name();
    println!(
        "[Bluesky DB DEBUG] Using SCAN+Filter for DID: {} in table: {}",
        bsky_did, current_table_name
    );

    let output = db
        .scan()
        .table_name(current_table_name)
        .filter_expression("bsky_did = :did AND item_type = :type")
        .expression_attribute_values(":did", AttributeValue::S(bsky_did.to_string()))
        .expression_attribute_values(":type", AttributeValue::S("BSKY_REG".to_string()))
        .send()
        .await;

    match &output {
        Ok(o) => println!(
            "[Bluesky DB DEBUG] Scan call succeeded. Items found: {}",
            o.items.as_ref().map_or(0, |i| i.len())
        ),
        Err(e) => println!("[Bluesky DB DEBUG] Scan call FAILED: {:?}", e),
    }

    let output = output.map_err(|e| anyhow::anyhow!("DynamoDB Scan error: {}", e))?;
    let item = output.items.unwrap_or_default().into_iter().next();

    match item {
        Some(item_map) => match from_item(item_map) {
            Ok(reg) => Ok(Some(reg)),
            Err(e) => {
                println!(
                    "[Bluesky DB DEBUG] Deserialization failed for DID {}: {:?}",
                    bsky_did, e
                );
                Err(anyhow::anyhow!("Deserialization error: {}", e))
            }
        },
        None => Ok(None),
    }
}

pub async fn get_twitch_reg_by_login(
    db: &aws_sdk_dynamodb::Client,
    twitch_login: &str,
) -> anyhow::Result<Option<TwitchRegistration>> {
    let output = db
        .query()
        .table_name(table_name())
        .index_name("GSI1")
        .key_condition_expression("GSI1PK = :pk AND GSI1SK = :sk")
        .expression_attribute_values(":pk", AttributeValue::S("TWITCH_LOOKUP".to_string()))
        .expression_attribute_values(
            ":sk",
            AttributeValue::S(format!("LOGIN#{}", twitch_login.to_lowercase())),
        )
        .send()
        .await?;

    let item = output.items.unwrap_or_default().pop();
    match item {
        Some(item_map) => Ok(from_item(item_map).ok()),
        None => Ok(None),
    }
}

pub async fn get_bsky_reg_by_handle(
    db: &aws_sdk_dynamodb::Client,
    bsky_handle: &str,
) -> anyhow::Result<Option<BlueskyRegistration>> {
    let output = db
        .query()
        .table_name(table_name())
        .index_name("GSI1")
        .key_condition_expression("GSI1PK = :pk AND GSI1SK = :sk")
        .expression_attribute_values(":pk", AttributeValue::S("BSKY_LOOKUP".to_string()))
        .expression_attribute_values(
            ":sk",
            AttributeValue::S(format!("HANDLE#{}", bsky_handle.to_lowercase())),
        )
        .send()
        .await?;

    let item = output.items.unwrap_or_default().pop(); // Expect 0 or 1
    match item {
        Some(item_map) => Ok(from_item(item_map).ok()),
        None => Ok(None),
    }
}

pub async fn save_rss_registration(
    db: &aws_sdk_dynamodb::Client,
    reg: RssRegistration,
) -> anyhow::Result<()> {
    let item = to_item(reg)?;
    db.put_item()
        .table_name(table_name())
        .set_item(Some(item))
        .send()
        .await?;
    Ok(())
}

pub async fn save_twitch_registration(
    db: &aws_sdk_dynamodb::Client,
    reg: TwitchRegistration,
) -> anyhow::Result<()> {
    let item = to_item(reg)?;
    db.put_item()
        .table_name(table_name())
        .set_item(Some(item))
        .send()
        .await?;
    Ok(())
}

pub async fn save_bluesky_registration(
    db: &aws_sdk_dynamodb::Client,
    reg: BlueskyRegistration,
) -> anyhow::Result<()> {
    let item = to_item(reg)?;
    db.put_item()
        .table_name(table_name())
        .set_item(Some(item))
        .send()
        .await?;
    Ok(())
}

pub async fn delete_registration(
    db: &aws_sdk_dynamodb::Client,
    pk: &str,
    sk: &str,
) -> anyhow::Result<()> {
    db.delete_item()
        .table_name(table_name())
        .key("PK", AttributeValue::S(pk.to_string()))
        .key("SK", AttributeValue::S(sk.to_string()))
        .send()
        .await?;
    println!("[DB] Deleted item with PK: {}, SK: {}", pk, sk);
    Ok(())
}

pub async fn save_server_config(
    db: &aws_sdk_dynamodb::Client,
    config: ServerConfig,
) -> anyhow::Result<()> {
    let item = to_item(config)?;
    db.put_item()
        .table_name(table_name())
        .set_item(Some(item))
        .send()
        .await?;
    Ok(())
}

pub async fn get_server_config(
    db: &aws_sdk_dynamodb::Client,
    guild_id: u64,
) -> anyhow::Result<Option<ServerConfig>> {
    let pk = format!("GUILD#{}", guild_id);
    let sk = "CONFIG#SETTINGS".to_string();

    let output = db
        .get_item()
        .table_name(table_name())
        .key("PK", AttributeValue::S(pk))
        .key("SK", AttributeValue::S(sk))
        .send()
        .await?;

    match output.item {
        Some(item) => Ok(from_item(item).ok()),
        None => Ok(None),
    }
}

pub async fn update_rss_last_post_guid(
    db: &aws_sdk_dynamodb::Client,
    pk: &str,
    sk: &str,
    last_post_guid: &str,
) -> anyhow::Result<()> {
    db.update_item()
        .table_name(table_name())
        .key("PK", AttributeValue::S(pk.to_string()))
        .key("SK", AttributeValue::S(sk.to_string()))
        .update_expression("SET last_post_guid = :guid")
        .expression_attribute_values(":guid", AttributeValue::S(last_post_guid.to_string()))
        .send()
        .await?;
    Ok(())
}

pub async fn save_tiktok_registration(
    db: &aws_sdk_dynamodb::Client,
    reg: TikTokRegistration,
) -> anyhow::Result<()> {
    let item = to_item(reg)?;
    db.put_item()
        .table_name(table_name())
        .set_item(Some(item))
        .send()
        .await?;
    Ok(())
}
