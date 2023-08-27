use std::collections::HashMap;

use chrono::{DateTime, Utc};
use eyre::{Result, WrapErr};
use serde_json::json;
use url::Url;

pub struct Client {
    endpoint_url: Url,
    access_token: String,
    http_client: reqwest::Client,
}

impl Client {
    pub fn from_url_and_token(endpoint_url: &Url, access_token: &str) -> Client {
        Client {
            endpoint_url: endpoint_url.to_owned(),
            access_token: access_token.to_owned(),
            http_client: reqwest::Client::new(),
        }
    }

    /// Make a GraphQL query to the Canvas API.
    async fn query(
        &self,
        query: &'static str,
        variables: HashMap<&'static str, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        // Set up the form parameters.
        let body = json!({
            "query": query,
            "variables": variables,
        });

        // Make the request using `reqwest`.
        let response = self
            .http_client
            .post(self.endpoint_url.as_str())
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&body)
            .send()
            .await
            .wrap_err("failed to make request to Canvas")?;

        #[derive(serde::Deserialize)]
        struct GraphqlError {
            message: String,
        }

        #[derive(serde::Deserialize)]
        struct GraphqlResponse {
            data: Option<serde_json::Value>,
            #[serde(default)]
            errors: Vec<GraphqlError>,
        }

        let response_body: GraphqlResponse = response
            .json()
            .await
            .wrap_err("failed to parse response from Canvas")?;

        // Return errors.
        for error in &response_body.errors {
            tracing::error!("Canvas GraphQL error: {}", error.message);
        }
        if !response_body.errors.is_empty() {
            let combined_error_message = response_body
                .errors
                .iter()
                .map(|error| error.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(eyre::eyre!(combined_error_message));
        }

        // Return the data.
        let data = response_body
            .data
            .ok_or_else(|| eyre::eyre!("Canvas GraphQL response did not contain data"))?;

        Ok(data)
    }

    /// List all the assignments in a given course.
    pub async fn get_assignments(&self, course_id: impl AsRef<str>) -> Result<Vec<Assignment>> {
        let course_id = course_id.as_ref();

        let query = r"
            query GetCourseAssignments($course_id: ID!) {
              course(id: $course_id) {
                id
                assignmentsConnection {
                  nodes {
                    _id
                    description
                    dueAt(applyOverrides: true)
                    htmlUrl
                    expectsSubmission
                    name
                  }
                }
              }
            }
        ";

        let variables: HashMap<&str, _> = [("course_id", json!(&course_id))].into_iter().collect();

        let response = self
            .query(query, variables)
            .await
            .wrap_err_with(|| format!("Failed to get assignments for course: {:?}", course_id))?;

        // Extract the data
        let data = response["course"]["assignmentsConnection"]["nodes"].clone();
        let data =
            serde_json::from_value(data).wrap_err("failed to deserialize assignment list")?;

        Ok(data)
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    #[serde(rename = "_id")]
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub due_at: DateTime<Utc>,
    pub html_url: Url,
    pub expects_submission: bool,
}
