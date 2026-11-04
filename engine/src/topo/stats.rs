//! Event statistics (placeholder, filled in below).
use super::witness::{G0In, Witness};
use crate::model::BoardModel;
pub fn collect(_m: &BoardModel, _w: &Witness, _g: Option<&G0In>) -> serde_json::Value {
    serde_json::Value::Null
}
