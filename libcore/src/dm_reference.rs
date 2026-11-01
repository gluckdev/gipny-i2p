// Wire/storage identity of a direct message, independent of SQL.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DmReference {
    pub author: [u8; 32],
    pub origin: u64,
    pub sent_at: i64,
}
