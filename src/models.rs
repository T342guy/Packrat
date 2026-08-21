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
    /// A pre-printed barcode stuck on this container, if any.
    pub barcode: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// When someone last confirmed the contents are still what's listed.
    pub checked_at: Option<String>,
    /// Days since that check; `None` when it has never been checked.
    pub days_since_check: Option<i64>,
    /// Seconds since that check, for wording finer than a day.
    pub seconds_since_check: Option<i64>,
    /// Days since the check, or since it was created if never checked.
    pub age_days: i64,
    /// The same span in seconds, so the UI can say "20 minutes ago".
    pub age_seconds: i64,
    /// Holds items and hasn't been verified within the staleness window.
    pub stale: bool,
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
    /// The product's own barcode (UPC/EAN) or one you assigned it.
    pub barcode: Option<String>,
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
    #[serde(default)]
    pub barcode: Option<String>,
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
    #[serde(default)]
    pub barcode: Option<String>,
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

/// What a scanned code turned out to be.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScanResult {
    Container { container: Box<ContainerDetail> },
    Item { item: Box<Item> },
    Unknown { code: String },
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub stale_containers: i64,
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

/// A child container together with what's in it, so a shelf can list its boxes
/// with their contents collapsed underneath each one.
#[derive(Debug, Serialize)]
pub struct ChildNode {
    #[serde(flatten)]
    pub container: Container,
    pub items: Vec<Item>,
    /// Containers nested one level further down (boxes inside a bin).
    pub child_count: i64,
}

#[derive(Debug, Serialize)]
pub struct ContainerDetail {
    pub container: Container,
    /// Ancestors, outermost first — used for breadcrumbs.
    pub ancestors: Vec<Container>,
    pub children: Vec<ChildNode>,
    pub items: Vec<Item>,
    /// Everything inside this container *and* its descendants.
    pub nested_item_count: i64,
    pub nested_total_quantity: i64,
}
