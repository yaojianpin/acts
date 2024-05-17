use crate::{utils, Result, Vars};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Deserialize, Serialize, Clone)]
pub struct ActionResult {
    pub start_time: i64,
    pub end_time: i64,
    outputs: Vars,
}

impl ActionResult {
    pub fn begin() -> Self {
        Self {
            start_time: utils::time::time_millis(),
            end_time: 0,
            outputs: Vars::new(),
        }
    }

    pub fn end(mut self) -> Result<Self> {
        self.end_time = utils::time::time_millis();
        Ok(self)
    }

    pub fn end_with_data<T>(mut self, name: &str, data: T) -> Result<Self>
    where
        T: Serialize + Clone,
    {
        self.end_time = utils::time::time_millis();
        self.outputs.set(name, data);
        Ok(self)
    }

    pub fn end_with_result<T>(mut self, result: Result<T>) -> Result<Self>
    where
        T: Serialize + Clone,
    {
        self.end_time = utils::time::time_millis();
        match result {
            Ok(v) => {
                self.outputs.set("data", v);
                Ok(self.clone())
            }
            Err(err) => Err(err),
        }
    }

    /// Get the workflow output vars
    pub fn outputs(&self) -> &Vars {
        &self.outputs
    }

    pub fn insert(&mut self, key: &str, value: JsonValue) {
        self.outputs.insert(key.to_string(), value);
    }

    /// How many time(million seconds) did a workflow cost
    pub fn cost(&self) -> i64 {
        self.end_time - self.start_time
    }

    pub fn attach<T>(&mut self, result: Result<T>)
    where
        T: Serialize + Clone,
    {
        match result {
            Ok(v) => {
                self.outputs.set("result", v);
            }
            Err(err) => {
                self.outputs.set("error", err.to_string());
            }
        }
    }
}

impl std::fmt::Debug for ActionResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("start_time", &self.start_time)
            .field("end_time", &self.end_time)
            .field("outputs", &self.outputs)
            .finish()
    }
}
