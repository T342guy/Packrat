use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Clone)]
pub struct Container {
    pub id: i64,
    pub code: String,
    pub name: String,
    /// area | shelf | cabinet | drawer | bin | box | bag | other
    pub kind: String,
    pub parent_id: Option<i64>,
    pub notes: String,
    pub photo_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    /// "Garage / North shelves / Camping bin" — computed, not stored.
    pub path: String,
    pub depth: i64,
    pub item_count: i64,
    pub total_quantity: i64,
    pub child_count: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct Item {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub quantity: i64,
    pub container_id: Option<i64>,
    pub photo_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub container_code: Option<String>,
    pub container_name: Option<String>,
    pub container_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ContainerInput {
    pub name: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub photo_id: Option<i64>,
    /// Optional custom label code; generated when absent.
    #[serde(default)]
    pub code: Option<String>,
}

fn default_kind() -> String {
    "box".to_string()
}

#[derive(Debug, Deserialize)]
pub struct ItemInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_quantity")]
    pub quantity: i64,
    #[serde(default)]
    pub container_id: Option<i64>,
    #[serde(default)]
    pub photo_id: Option<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_quantity() -> i64 {
    1
}

#[derive(Debug, Deserialize)]
pub struct MoveInput {
    pub container_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct QuantityInput {
    pub delta: i64,
}

#[derive(Debug, Serialize)]
pub struct TagCount {
    pub name: String,
    pub item_count: i64,
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub items: i64,
    pub total_quantity: i64,
    pub containers: i64,
    pub boxes: i64,
    pub tags: i64,
    pub photos: i64,
    pub unfiled_items: i64,
    pub empty_containers: i64,
    pub database_bytes: i64,
}

#[derive(Debug, Serialize)]
pub struct ContainerDetail {
    pub container: Container,
    /// Ancestors, outermost first — used for breadcrumbs.
    pub ancestors: Vec<Container>,
    pub children: Vec<Container>,
    pub items: Vec<Item>,
    /// Everything inside this container *and* its descendants.
    pub nested_item_count: i64,
    pub nested_total_quantity: i64,
}
