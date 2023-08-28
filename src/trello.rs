use std::collections::HashMap;

use chrono::{DateTime, Utc};
use eyre::{Result, WrapErr};
use reqwest::Method;
use serde_json::json;

pub struct Client {
    pub api_key: String,
    pub api_token: String,
    pub http_client: reqwest::Client,
    pub base_url: String,
}

impl Client {
    pub fn from_key_and_token(api_key: impl AsRef<str>, api_token: impl AsRef<str>) -> Client {
        Client {
            api_key: api_key.as_ref().to_owned(),
            api_token: api_token.as_ref().to_owned(),
            http_client: reqwest::Client::new(),
            base_url: "https://api.trello.com/1/".into(),
        }
    }

    pub fn req(&self, method: reqwest::Method, url: impl AsRef<str>) -> reqwest::RequestBuilder {
        self.http_client.request(method, url.as_ref()).header(
            "Authorization",
            format!(
                r#"OAuth oauth_consumer_key="{}", oauth_token="{}""#,
                self.api_key, self.api_token
            ),
        )
    }

    pub async fn get_board_contents(&self, board_id: &str) -> Result<Board> {
        let url = format!("{}/boards/{}", self.base_url, board_id);
        let resp = self
            .req(Method::GET, url)
            .query(&[
                ("cards", "all"),
                ("card_customFieldItems", "true"),
                ("customFields", "true"),
                ("labels", "all"),
                ("lists", "all"),
            ])
            .send()
            .await
            .wrap_err_with(|| format!("Failed to get contents of board: {:?}", board_id))?;

        let body: Board = resp
            .json()
            .await
            .wrap_err("Failed to parse response body")?;

        Ok(body)
    }

    pub async fn update_card<T, K, V>(
        &self,
        card_id: &str,
        patch: impl IntoIterator<Item = T>,
    ) -> Result<()>
    where
        K: AsRef<str>,
        V: AsRef<str>,
        HashMap<K, V>: FromIterator<T>,
    {
        let patch = patch.into_iter().collect::<HashMap<_, _>>();
        let patch = patch
            .into_iter()
            .map(|(k, v)| (k.as_ref().to_owned(), v.as_ref().to_owned()))
            .collect::<HashMap<_, _>>();
        let url = format!("{}/cards/{}", self.base_url, card_id);
        self.req(Method::PUT, url)
            .query(&patch)
            .send()
            .await
            .wrap_err_with(|| format!("Failed to update card: {:?}", card_id))?;
        Ok(())
    }

    pub async fn create_card(&self, list_id: &str, create_card: CreateCard) -> Result<Card> {
        let url = format!("{}/cards", self.base_url);
        let resp = self
            .req(Method::POST, url)
            .query(&[
                ("idList", list_id),
                ("name", &create_card.name),
                ("desc", &create_card.desc),
                ("due", &create_card.due.to_rfc3339()),
                ("due_complete", &create_card.due_complete.to_string()),
                ("idLabels", &create_card.label_ids.join(",")),
            ])
            .send()
            .await
            .wrap_err_with(|| format!("Failed to create card: {:?}", create_card.name))?;

        let body: Card = resp
            .json()
            .await
            .wrap_err("Failed to parse response body")?;

        Ok(body)
    }

    pub async fn set_card_custom_field(
        &self,
        card_id: &str,
        field_id: &str,
        field_value: CustomFieldValue,
    ) -> Result<()> {
        let url = format!(
            "{}/cards/{}/customField/{}/item",
            self.base_url, card_id, field_id
        );
        self.req(Method::PUT, url)
            .json(&json!({"value": field_value}))
            .send()
            .await
            .wrap_err_with(|| format!("Failed to set custom field of card: {:?}", card_id))?;
        Ok(())
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub cards: Vec<Card>,
    pub custom_fields: Vec<CustomFieldDesc>,
    pub labels: Vec<Label>,
    pub lists: Vec<List>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomFieldDesc {
    pub id: String,
    pub name: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub due: Option<DateTime<Utc>>,
    pub due_complete: bool,
    pub labels: Vec<Label>,
    #[serde(default)]
    pub custom_field_items: Vec<CustomFieldItem>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub id: String,
    pub name: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct List {
    pub id: String,
    pub name: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomFieldItem {
    pub id: String,
    pub value: CustomFieldValue,
    pub id_custom_field: String,
}

impl CustomFieldItem {
    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            CustomFieldValue::Text { text } => Some(text),
            _ => None,
        }
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum CustomFieldValue {
    Text {
        text: String,
    },
    Other {
        #[serde(flatten)]
        value: serde_json::Value,
    },
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCard {
    pub name: String,
    pub desc: String,
    pub due: DateTime<Utc>,
    pub due_complete: bool,
    pub label_ids: Vec<String>,
}
