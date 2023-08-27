use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub trello: Trello,
    pub canvas: Canvas,

    pub mapping: Vec<Mapping>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Trello {
    pub board_id: String,
    pub add_to_list: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Canvas {
    pub graphql_endpoint: Url,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Mapping {
    pub canvas_course_id: String,
    pub trello_label_name: String,
}
